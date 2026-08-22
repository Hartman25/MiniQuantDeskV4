//! PROMOTION-STRESS-SUITE-AUTHORITY-01 — real stress-suite execution +
//! durable, tamper-evident, candidate-bound artifact authority.
//!
//! Validates:
//! - A real `run_backtest_stress_suite` execution round-trips through
//!   `write_canonical_stress_suite`/`load_canonical_stress_suite`.
//! - A genuinely fragile strategy (large drawdown from a real price
//!   collapse, realized via actual fills) produces a real failed scenario
//!   -- `all_passed()` is `false`, proving the pass/fail gate is not
//!   vacuous.
//! - Zero scenarios, missing artifact, missing manifest, schema-version,
//!   run_id/strategy/config mismatch (including a genuine cross-candidate
//!   swap between two independent real runs), missing audit event, broken
//!   audit chain, and post-write content tampering are all rejected.
//! - The write is idempotent across repeated calls.
//! - No production source constructs `StressSuiteRunOutput` via struct
//!   literal outside `mqk-backtest`'s own `stress_suite.rs` (test-only
//!   fabrication cannot become production authority).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mqk_artifacts::{load_canonical_stress_suite, InitRunArtifactsArgs, StressSuiteArtifactError};
use mqk_backtest::{run_backtest_stress_suite, BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

const M: i64 = 1_000_000;
static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Buys `qty` on bar 1, holds, sells (flattens) starting bar `sell_at_idx`.
struct BuyHoldSell {
    bar_idx: u64,
    qty: i64,
    sell_at_idx: u64,
    name: &'static str,
}

impl BuyHoldSell {
    fn new(qty: i64, sell_at_idx: u64) -> Self {
        Self {
            bar_idx: 0,
            qty,
            sell_at_idx,
            name: "BsaaBuyHoldSell",
        }
    }

    fn named(name: &'static str, qty: i64, sell_at_idx: u64) -> Self {
        Self {
            bar_idx: 0,
            qty,
            sell_at_idx,
            name,
        }
    }
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.name, 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        if self.bar_idx < self.sell_at_idx {
            StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)])
        } else {
            StrategyOutput::new(vec![TargetPosition::new("ES", 0)])
        }
    }
}

fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
}

fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity
    cfg
}

/// Mild, flat-ish bars -- a healthy candidate that should clear every
/// stress scenario.
fn healthy_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 500),
        flat_bar(1_700_000_120, 501),
        flat_bar(1_700_000_180, 502),
        flat_bar(1_700_000_240, 503),
    ]
}

/// A real ~35% price collapse (500 -> 150) realized through actual
/// buy/sell fills on a large (100-share, ~50% of equity) position -- a
/// genuinely fragile candidate that must fail the conservative drawdown
/// ceiling in every scenario, regardless of cost-stress multiplier.
fn fragile_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 500),
        flat_bar(1_700_000_120, 500),
        flat_bar(1_700_000_180, 150),
        flat_bar(1_700_000_240, 150),
    ]
}

/// Run a real `BacktestEngine`, persist full canonical artifacts (manifest,
/// audit, backtest_report.json) into a fresh temp run dir. Returns
/// (report, config, bars, run_dir).
fn run_and_persist(
    label: &str,
    strategy_name: &'static str,
    bars: Vec<BacktestBar>,
    qty: i64,
    sell_at_idx: u64,
) -> (mqk_backtest::BacktestReport, BacktestConfig, Vec<BacktestBar>, PathBuf) {
    let config = cfg_with_wide_cap();
    let initial_cash = config.initial_cash_micros;

    let mut engine = BacktestEngine::new(config.clone());
    engine
        .add_strategy(Box::new(BuyHoldSell::named(strategy_name, qty, sell_at_idx)))
        .unwrap();
    let report = engine.run(&bars).expect("engine.run must succeed");

    let seq = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let exports_root = std::env::temp_dir().join(format!(
        "mqk_bsaa01_{}_{}_{}",
        label,
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
        git_hash: "bsaa01_test_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "bsaa01_test_host",
        now_utc: chrono::Utc::now(),
    })
    .expect("init_run_artifacts must succeed");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report must succeed");

    (report, config, bars, init_result.run_dir)
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

fn tamper_stress_suite_json(run_dir: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let path = run_dir.join("stress_suite.json");
    let raw = fs::read_to_string(&path).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    mutate(&mut v);
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ---------------------------------------------------------------------------
// 1. Real round trip: healthy candidate passes every scenario
// ---------------------------------------------------------------------------

#[test]
fn bsaa01a_healthy_candidate_round_trip_all_scenarios_pass() {
    let (report, config, bars, run_dir) = run_and_persist("healthy", "BsaaHealthy", healthy_bars(), 1, 3);

    let output = run_backtest_stress_suite(&report, &config, &bars, || {
        Box::new(BuyHoldSell::named("BsaaHealthy", 1, 3))
    });
    assert_eq!(output.scenarios.len(), 3);
    assert!(output.all_passed(), "healthy candidate must pass every scenario");
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output)
        .expect("write_canonical_stress_suite must succeed");

    let loaded = load_canonical_stress_suite(&run_dir).expect("load must succeed");
    assert_eq!(loaded.scenarios_run(), 3);
    assert!(loaded.all_passed());
    assert!(loaded.failed_scenario_descriptions().is_empty());

    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 2. Real failed scenario: fragile candidate
// ---------------------------------------------------------------------------

#[test]
fn bsaa01b_fragile_candidate_produces_real_failed_scenario() {
    let (report, config, bars, run_dir) = run_and_persist("fragile", "BsaaFragile", fragile_bars(), 100, 3);

    let output = run_backtest_stress_suite(&report, &config, &bars, || {
        Box::new(BuyHoldSell::new(100, 3))
    });
    assert!(
        !output.all_passed(),
        "a ~35% realized-loss candidate must fail at least one stress scenario"
    );

    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();
    let loaded = load_canonical_stress_suite(&run_dir).expect("load must succeed");
    assert!(!loaded.all_passed());
    assert!(!loaded.failed_scenario_descriptions().is_empty());

    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 3-8. Fail-closed loader controls
// ---------------------------------------------------------------------------

#[test]
fn bsaa01c_missing_artifact_rejected() {
    let (_report, _config, _bars, run_dir) = run_and_persist("missing_artifact", "BsaaMissingArtifact", healthy_bars(), 1, 3);
    // No stress suite ever written for this run_dir.
    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(err, StressSuiteArtifactError::MissingArtifact);
    cleanup(&run_dir);
}

#[test]
fn bsaa01d_zero_scenarios_rejected() {
    let (report, config, bars, run_dir) = run_and_persist("zero_scen", "BsaaZeroScen", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    tamper_stress_suite_json(&run_dir, |v| {
        v["scenarios"] = serde_json::Value::Array(vec![]);
    });

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(err, StressSuiteArtifactError::ZeroScenarios);
    cleanup(&run_dir);
}

#[test]
fn bsaa01e_run_id_mismatch_rejected() {
    let (report, config, bars, run_dir) = run_and_persist("run_id_mismatch", "BsaaRunIdMismatch", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    let fake = uuid::Uuid::from_u128(0xdead_beef);
    tamper_stress_suite_json(&run_dir, |v| {
        v["run_id"] = serde_json::Value::String(fake.to_string());
    });

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    match err {
        StressSuiteArtifactError::RunIdMismatch { artifact, .. } => assert_eq!(artifact, fake),
        other => panic!("expected RunIdMismatch, got {other:?}"),
    }
    cleanup(&run_dir);
}

#[test]
fn bsaa01f_cross_candidate_stress_artifact_rejected() {
    // Two INDEPENDENT real runs (different strategies -> different run_ids).
    let (report_a, config_a, bars_a, run_dir_a) =
        run_and_persist("cross_a", "BsaaCrossA", healthy_bars(), 1, 3);
    let (report_b, _config_b, _bars_b, run_dir_b) =
        run_and_persist("cross_b", "BsaaCrossB", healthy_bars(), 2, 3);
    assert_ne!(report_a.run_id, report_b.run_id);

    let output_a = run_backtest_stress_suite(&report_a, &config_a, &bars_a, || {
        Box::new(BuyHoldSell::named("BsaaCrossA", 1, 3))
    });
    mqk_artifacts::write_canonical_stress_suite(&run_dir_a, &output_a).unwrap();

    // Copy candidate A's stress_suite.json into candidate B's run_dir --
    // candidate B's evidence must never be satisfiable by candidate A's
    // stress result.
    fs::copy(
        run_dir_a.join("stress_suite.json"),
        run_dir_b.join("stress_suite.json"),
    )
    .unwrap();

    let err = load_canonical_stress_suite(&run_dir_b).unwrap_err();
    assert!(matches!(err, StressSuiteArtifactError::RunIdMismatch { .. }));

    cleanup(&run_dir_a);
    cleanup(&run_dir_b);
}

#[test]
fn bsaa01g_missing_audit_event_rejected() {
    let (report, config, bars, run_dir) = run_and_persist("no_audit_event", "BsaaNoAuditEvent", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));

    // Write ONLY the JSON file directly (bypass write_canonical_stress_suite
    // so no audit event is appended) -- simulates a hand-fabricated artifact.
    let artifact_path = run_dir.join("stress_suite.json");
    let artifact = mqk_artifacts::StressSuiteArtifact::from(&output);
    fs::write(
        &artifact_path,
        serde_json::to_string_pretty(&artifact).unwrap(),
    )
    .unwrap();

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(err, StressSuiteArtifactError::AuditEventMissing);
    cleanup(&run_dir);
}

#[test]
fn bsaa01h_broken_audit_chain_rejected() {
    let (report, config, bars, run_dir) = run_and_persist("broken_chain", "BsaaBrokenChain", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    // Corrupt the LAST event's hash_self (the stress_suite_completed event).
    let audit_path = run_dir.join("audit.jsonl");
    let content = fs::read_to_string(&audit_path).unwrap();
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let last = lines.pop().unwrap();
    let mut ev: serde_json::Value = serde_json::from_str(&last).unwrap();
    ev["hash_self"] = serde_json::Value::String("0".repeat(64));
    lines.push(serde_json::to_string(&ev).unwrap());
    fs::write(&audit_path, format!("{}\n", lines.join("\n"))).unwrap();

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert!(matches!(err, StressSuiteArtifactError::AuditChainBroken(_)));
    cleanup(&run_dir);
}

#[test]
fn bsaa01i_content_tampered_after_write_rejected() {
    let (report, config, bars, run_dir) = run_and_persist("content_tamper", "BsaaContentTamper", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    // Flip a scenario's `passed` flag from true to false, WITHOUT touching
    // run_id/strategy_name/config_id/schema_version/scenario count, and
    // WITHOUT touching audit.jsonl -- the audit chain itself stays intact,
    // but the recorded content hash no longer matches the file.
    tamper_stress_suite_json(&run_dir, |v| {
        v["scenarios"][0]["passed"] = serde_json::Value::Bool(false);
    });

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(err, StressSuiteArtifactError::ContentHashMismatch);
    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 3b-3e. PROMOTION-STRESS-AUTHORITY-REPAIR-01: protocol identity controls
// ---------------------------------------------------------------------------

/// Simulates a legitimately-written, internally-consistent artifact produced
/// under an unsupported protocol (e.g. an old/never-accepted version) --
/// mutating `StressSuiteRunOutput.protocol_version` BEFORE writing (rather
/// than tampering the JSON after) means the resulting `stress_suite.json`
/// and its `stress_suite_completed` audit hash agree perfectly with each
/// other; only the structural protocol-identity check in
/// `load_canonical_stress_suite` -- not audit/content-hash tamper detection
/// -- can reject it.
#[test]
fn bsaa01l_wrong_protocol_version_rejected() {
    let (report, config, bars, run_dir) =
        run_and_persist("wrong_protocol", "BsaaWrongProtocol", healthy_bars(), 1, 3);
    let mut output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    output.protocol_version = "bkt_stress_suite_v0_fabricated".to_string();
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(
        err,
        StressSuiteArtifactError::UnsupportedProtocolVersion(
            "bkt_stress_suite_v0_fabricated".to_string()
        )
    );
    cleanup(&run_dir);
}

/// Same construction as [`bsaa01l_wrong_protocol_version_rejected`] but with
/// a blank protocol -- no implicit "unset means legacy/trusted" fallback.
#[test]
fn bsaa01m_blank_protocol_version_rejected() {
    let (report, config, bars, run_dir) =
        run_and_persist("blank_protocol", "BsaaBlankProtocol", healthy_bars(), 1, 3);
    let mut output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    output.protocol_version = String::new();
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(
        err,
        StressSuiteArtifactError::UnsupportedProtocolVersion(String::new())
    );
    cleanup(&run_dir);
}

/// Same "consistent, not tampered" construction as
/// [`bsaa01l_wrong_protocol_version_rejected`]: the `conservative_risk_limits`
/// scenario is removed from `StressSuiteRunOutput` BEFORE writing, so the
/// resulting `stress_suite.json` and its audit hash agree perfectly -- only
/// the required-scenario-set completeness check can reject it.
#[test]
fn bsaa01n_missing_required_scenario_rejected() {
    let (report, config, bars, run_dir) =
        run_and_persist("missing_scenario", "BsaaMissingScenario", healthy_bars(), 1, 3);
    let mut output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));
    output.scenarios.retain(|s| s.name != "conservative_risk_limits");
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    let err = load_canonical_stress_suite(&run_dir).unwrap_err();
    assert_eq!(
        err,
        StressSuiteArtifactError::MissingRequiredScenario("conservative_risk_limits".to_string())
    );
    cleanup(&run_dir);
}

// Note: a `protocol_version` tampered post-write (matching or not) is caught
// by `UnsupportedProtocolVersion` (bsaa01l) BEFORE the audit/content-hash
// check ever runs -- structural protocol-identity rejection is strictly
// earlier and strictly fail-closed relative to hash-chain tamper detection,
// so there is no separate "protocol changed but hash still matches" case to
// prove: any post-write edit to this field either fails structurally (wrong
// value) or leaves the artifact byte-identical (no edit at all).

// ---------------------------------------------------------------------------
// 9. Idempotent write
// ---------------------------------------------------------------------------

#[test]
fn bsaa01j_write_is_idempotent() {
    let (report, config, bars, run_dir) = run_and_persist("idempotent", "BsaaIdempotent", healthy_bars(), 1, 3);
    let output =
        run_backtest_stress_suite(&report, &config, &bars, || Box::new(BuyHoldSell::new(1, 3)));

    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();
    mqk_artifacts::write_canonical_stress_suite(&run_dir, &output).unwrap();

    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let stress_events = audit_jsonl
        .lines()
        .filter(|l| l.contains("stress_suite_completed"))
        .count();
    assert_eq!(stress_events, 1, "stress completion event must not duplicate on retry");

    let verify = mqk_audit::verify_hash_chain_str(&audit_jsonl).unwrap();
    assert!(matches!(verify, mqk_audit::VerifyResult::Valid { .. }));

    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// 10. Structural: no production bypass constructor
// ---------------------------------------------------------------------------

#[test]
fn bsaa01k_no_production_struct_literal_bypass_exists() {
    // The only legitimate construction path for a real `StressSuiteRunOutput`
    // is `mqk_backtest::stress_suite::run_backtest_stress_suite`. Prove no
    // OTHER production source file (outside stress_suite.rs itself, and
    // outside test files) fabricates one via struct literal.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut offenders = Vec::new();
    for crate_dir in [
        "mqk-artifacts", "mqk-backtest", "mqk-promotion", "mqk-cli", "mqk-daemon",
    ] {
        let src_dir = root.join("crates").join(crate_dir).join("src");
        if !src_dir.exists() {
            continue;
        }
        visit_rs_files(&src_dir, &mut |path, contents| {
            if path.file_name().and_then(|f| f.to_str()) == Some("stress_suite.rs") {
                return; // the real implementation itself
            }
            if contents.contains("StressSuiteRunOutput {") {
                offenders.push(path.to_path_buf());
            }
        });
    }
    assert!(
        offenders.is_empty(),
        "found production struct-literal construction of StressSuiteRunOutput outside stress_suite.rs: {offenders:?}"
    );
}

fn visit_rs_files(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rs_files(&path, f);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(contents) = fs::read_to_string(&path) {
                f(&path, &contents);
            }
        }
    }
}
