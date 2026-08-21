//! BKT-PROMOTION-ARTIFACT-AUTHORITY-01 — canonical `backtest_report.json`
//! authority + real-artifact `lock_artifact_from_str` acceptance.
//!
//! Validates:
//! - A real `BacktestEngine` run, persisted via `init_run_artifacts` +
//!   `write_backtest_report`, round-trips losslessly through
//!   `load_canonical_backtest_report` (`loaded == original`).
//! - `run_id` / `strategy_name` / `config_id` / `execution_model_id`
//!   mismatches between `backtest_report.json` and `manifest.json` are each
//!   independently rejected (single-field mutation proof: every tamper test
//!   changes exactly one field, all else held byte-identical).
//! - A missing/malformed/unsupported-schema-version `backtest_report.json`
//!   fails closed and is never reconstructed from `metrics.json`/CSVs.
//! - The real manifest + audit log emitted by a genuine backtest run are
//!   accepted by `mqk_promotion::lock_artifact_from_str`.
//! - An empty or broken audit chain is rejected by the same real-artifact
//!   path (not just synthetic fixtures).
//! - The completion audit event is idempotent across repeated
//!   `write_backtest_report` calls into the same run directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mqk_artifacts::{
    load_canonical_backtest_report, BacktestReportArtifactError, InitRunArtifactsArgs,
};
use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestReport};
use mqk_promotion::{lock_artifact_from_str, LockError};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};
use uuid::Uuid;

const M: i64 = 1_000_000;
static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Real engine fixture
// ---------------------------------------------------------------------------

struct BuyHoldSell {
    bar_idx: u64,
    qty: i64,
}

impl BuyHoldSell {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BraaBuyHoldSell", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 | 2 => StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)]),
            _ => StrategyOutput::new(vec![TargetPosition::new("ES", 0)]),
        }
    }
}

fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
}

fn bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 4_500),
        flat_bar(1_700_000_120, 4_505),
        flat_bar(1_700_000_180, 4_510),
        flat_bar(1_700_000_240, 4_515),
    ]
}

fn cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity
    cfg
}

/// Run a real `BacktestEngine`, persist full canonical artifacts (manifest,
/// audit, backtest_report.json, etc.) into a fresh temp run dir, and return
/// (report, run_dir).
fn run_and_persist() -> (BacktestReport, PathBuf) {
    let bars = bars();
    let config = cfg();
    let initial_cash = config.initial_cash_micros;

    let mut engine = BacktestEngine::new(config);
    engine.add_strategy(Box::new(BuyHoldSell::new(10))).unwrap();
    let report = engine.run(&bars).expect("engine.run must succeed");
    assert!(
        !report.execution_model_id.is_empty(),
        "fixture precondition: real engine reports must carry a non-empty execution_model_id"
    );

    let seq = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let exports_root = std::env::temp_dir().join(format!(
        "mqk_braa01_{}_{}",
        std::process::id(),
        seq
    ));
    let _ = fs::remove_dir_all(&exports_root);

    let config_hash = report.config_id.to_string();
    let init_result = mqk_artifacts::init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &exports_root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "braa01_test_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "braa01_test_host",
        now_utc: chrono::Utc::now(),
    })
    .expect("init_run_artifacts must succeed");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report must succeed");

    (report, init_result.run_dir)
}

/// Read `backtest_report.json`, apply `mutate`, write it back.
fn tamper_canonical_report(run_dir: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let path = run_dir.join("backtest_report.json");
    let raw = fs::read_to_string(&path).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    mutate(&mut v);
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// 1. Real engine round-trip
// ---------------------------------------------------------------------------

#[test]
fn braa01a_real_engine_round_trip_is_semantically_equal() {
    let (report, run_dir) = run_and_persist();
    assert!(run_dir.join("backtest_report.json").exists());

    let loaded = load_canonical_backtest_report(&run_dir).expect("load must succeed");
    assert_eq!(loaded, report, "round-tripped report must equal original");

    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 2-5. Single-field mismatch rejection (mutation proof: exactly one field
//      changed per test, everything else byte-identical).
// ---------------------------------------------------------------------------

#[test]
fn braa01b_run_id_mismatch_rejected() {
    let (_report, run_dir) = run_and_persist();
    let fake = Uuid::from_u128(0xdead_beef);
    tamper_canonical_report(&run_dir, |v| {
        v["run_id"] = serde_json::Value::String(fake.to_string());
    });

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    match err {
        BacktestReportArtifactError::RunIdMismatch { report, .. } => assert_eq!(report, fake),
        other => panic!("expected RunIdMismatch, got {other:?}"),
    }
    cleanup(&run_dir);
}

#[test]
fn braa01c_strategy_name_mismatch_rejected() {
    let (_report, run_dir) = run_and_persist();
    tamper_canonical_report(&run_dir, |v| {
        v["strategy_name"] = serde_json::Value::String("some_other_strategy".to_string());
    });

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    assert!(matches!(
        err,
        BacktestReportArtifactError::StrategyNameMismatch { .. }
    ));
    cleanup(&run_dir);
}

#[test]
fn braa01d_config_id_mismatch_rejected() {
    let (_report, run_dir) = run_and_persist();
    let fake = Uuid::from_u128(0xc0ffee);
    tamper_canonical_report(&run_dir, |v| {
        v["config_id"] = serde_json::Value::String(fake.to_string());
    });

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    match err {
        BacktestReportArtifactError::ConfigIdMismatch {
            report_config_id, ..
        } => assert_eq!(report_config_id, fake.to_string()),
        other => panic!("expected ConfigIdMismatch, got {other:?}"),
    }
    cleanup(&run_dir);
}

#[test]
fn braa01e_execution_model_mismatch_rejected() {
    let (_report, run_dir) = run_and_persist();
    tamper_canonical_report(&run_dir, |v| {
        v["execution_model_id"] = serde_json::Value::String("some_other_execution_model".to_string());
    });

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    assert!(matches!(
        err,
        BacktestReportArtifactError::ExecutionModelMismatch { .. }
    ));
    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 6-8. Missing / malformed / unsupported-schema-version
// ---------------------------------------------------------------------------

#[test]
fn braa01f_missing_canonical_report_rejected_not_reconstructed() {
    let (_report, run_dir) = run_and_persist();
    // Old-style artifact: manifest + metrics.json present, canonical report
    // absent (simulates an artifact written before this schema existed).
    assert!(run_dir.join("metrics.json").exists());
    fs::remove_file(run_dir.join("backtest_report.json")).unwrap();

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    assert_eq!(err, BacktestReportArtifactError::MissingCanonicalReport);
    cleanup(&run_dir);
}

#[test]
fn braa01g_malformed_json_rejected() {
    let (_report, run_dir) = run_and_persist();
    fs::write(run_dir.join("backtest_report.json"), "{ this is not valid json").unwrap();

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    assert!(matches!(err, BacktestReportArtifactError::MalformedJson(_)));
    cleanup(&run_dir);
}

#[test]
fn braa01h_unsupported_schema_version_rejected() {
    let (_report, run_dir) = run_and_persist();
    tamper_canonical_report(&run_dir, |v| {
        v["schema_version"] = serde_json::Value::Number(999.into());
    });

    let err = load_canonical_backtest_report(&run_dir).unwrap_err();
    assert_eq!(
        err,
        BacktestReportArtifactError::UnsupportedSchemaVersion(999)
    );
    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 9-11. Real manifest + audit accepted by `lock_artifact_from_str`
// ---------------------------------------------------------------------------

#[test]
fn braa01i_real_emitted_manifest_and_audit_accepted_by_lock_artifact_from_str() {
    let (_report, run_dir) = run_and_persist();
    let manifest_json = fs::read_to_string(run_dir.join("manifest.json")).unwrap();
    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    assert!(
        !audit_jsonl.trim().is_empty(),
        "real backtest run must produce a non-empty audit.jsonl (completion event)"
    );

    let lock = lock_artifact_from_str(&manifest_json, &audit_jsonl)
        .expect("real emitted manifest + audit must be accepted");
    assert_eq!(lock.audit_lines_verified, 1);
    cleanup(&run_dir);
}

#[test]
fn braa01j_empty_audit_from_real_run_dir_rejected() {
    // init_run_artifacts alone (no write_backtest_report) leaves audit.jsonl
    // as the empty placeholder -- exactly the pre-PATCH-A defect state.
    let seq = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let exports_root = std::env::temp_dir().join(format!(
        "mqk_braa01j_{}_{}",
        std::process::id(),
        seq
    ));
    let _ = fs::remove_dir_all(&exports_root);
    let init_result = mqk_artifacts::init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &exports_root,
        schema_version: 1,
        run_id: Uuid::from_u128(1),
        strategy_name: "braa01j",
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: None,
        git_hash: "githash",
        config_hash: "confighash",
        host_fingerprint: "host",
        now_utc: chrono::Utc::now(),
    })
    .unwrap();

    let manifest_json = fs::read_to_string(&init_result.manifest_path).unwrap();
    let audit_jsonl = fs::read_to_string(init_result.run_dir.join("audit.jsonl")).unwrap();
    assert!(audit_jsonl.is_empty());

    let err = lock_artifact_from_str(&manifest_json, &audit_jsonl).unwrap_err();
    assert_eq!(err, LockError::AuditEmpty);
    cleanup(&init_result.run_dir);
}

#[test]
fn braa01k_broken_audit_chain_from_real_run_dir_rejected() {
    let (_report, run_dir) = run_and_persist();
    let manifest_json = fs::read_to_string(run_dir.join("manifest.json")).unwrap();
    let mut audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();

    // Corrupt the single completion event's hash_self so the chain no
    // longer verifies.
    let mut ev: serde_json::Value = serde_json::from_str(audit_jsonl.trim()).unwrap();
    ev["hash_self"] = serde_json::Value::String("0".repeat(64));
    audit_jsonl = format!("{}\n", serde_json::to_string(&ev).unwrap());

    let err = lock_artifact_from_str(&manifest_json, &audit_jsonl).unwrap_err();
    assert!(matches!(err, LockError::AuditChainBroken { .. }));
    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 12. Idempotent completion audit event (restart/replay safety)
// ---------------------------------------------------------------------------

#[test]
fn braa01l_completion_audit_event_is_idempotent_across_repeated_writes() {
    let (report, run_dir) = run_and_persist();
    let initial_cash = cfg().initial_cash_micros;

    // Re-run write_backtest_report into the SAME run_dir with the SAME
    // report (simulates a retried/resumed write) -- must not append a
    // second, chain-breaking event.
    mqk_artifacts::write_backtest_report(&run_dir, &report, initial_cash)
        .expect("second write_backtest_report call must succeed");

    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let event_count = audit_jsonl.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        event_count, 1,
        "completion audit event must not be duplicated on retry"
    );

    let verify = mqk_audit::verify_hash_chain_str(&audit_jsonl).expect("verify must not error");
    assert!(matches!(
        verify,
        mqk_audit::VerifyResult::Valid { lines: 1 }
    ));

    cleanup(&run_dir);
}
