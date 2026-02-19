//! PATCH 24: Replay determinism parity scenario.
//!
//! Asserts:
//!   - Running the SAME orchestrator scenario twice produces deterministic
//!     artifacts that match on all non-varying fields.
//!   - config_hash is identical across runs.
//!   - Audit hash chain is valid for both runs (mqk_audit::verify_hash_chain).
//!   - Fill counts, equity curve values, and broker event payloads match.
//!   - Fails if any deterministic field mismatches between runs.

use anyhow::Result;
use mqk_audit::{verify_hash_chain, VerifyResult};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorReport};
use std::fs;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Deterministic strategy: buys bar 1, holds, sells bar 4.
// ---------------------------------------------------------------------------

struct DeterministicStrategy {
    bar_num: u64,
}

impl DeterministicStrategy {
    fn new() -> Self {
        Self { bar_num: 0 }
    }
}

impl Strategy for DeterministicStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Deterministic", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_num += 1;
        match self.bar_num {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            2 | 3 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            4 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_bars(n: usize) -> Vec<OrchestratorBar> {
    (0..n)
        .map(|i| {
            let ts = 1_700_000_000 + (i as i64) * 60;
            let price = 100_000_000 + (i as i64) * 100_000;
            OrchestratorBar {
                symbol: "SPY".to_string(),
                end_ts: ts,
                open_micros: price - 50_000,
                high_micros: price + 100_000,
                low_micros: price - 100_000,
                close_micros: price,
                volume: 1000,
                is_complete: true,
                day_id: 20250101,
            }
        })
        .collect()
}

/// Run one orchestrator scenario and return the report.
fn run_scenario(exports_root: &std::path::Path) -> Result<OrchestratorReport> {
    let bars = make_bars(6);
    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(DeterministicStrategy::new()))?;
    orch.run(&bars, exports_root)
}

/// Parse manifest.json from a run directory into serde_json::Value.
fn parse_manifest(run_dir: &std::path::Path) -> serde_json::Value {
    let manifest_path = run_dir.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path).expect("manifest.json should be readable");
    serde_json::from_str(&raw).expect("manifest.json should parse as valid JSON")
}

/// Parse audit.jsonl into a Vec of serde_json::Value.
fn parse_audit_events(run_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let audit_path = run_dir.join("audit.jsonl");
    let content = fs::read_to_string(&audit_path).expect("audit.jsonl should be readable");
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("invalid JSON in audit line: {e}"))
        })
        .collect()
}

/// Extract deterministic broker event payloads (topic, event_type, and
/// payload fields excluding order_id/fill_id which contain sequential IDs
/// that are deterministic within a run but differ between runs due to
/// separate Orchestrator instances).
fn extract_broker_event_signatures(events: &[serde_json::Value]) -> Vec<(String, String, String, i64)> {
    events
        .iter()
        .filter(|ev| ev["topic"].as_str() == Some("broker"))
        .map(|ev| {
            let event_type = ev["event_type"].as_str().unwrap_or("").to_string();
            let symbol = ev["payload"]["symbol"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let side = ev["payload"]["side"].as_str().unwrap_or("").to_string();
            let qty = ev["payload"]["qty"].as_i64().unwrap_or(0);
            (event_type, symbol, side, qty)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn determinism_config_hash_matches_across_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    let manifest1 = parse_manifest(&report1.run_dir);
    let manifest2 = parse_manifest(&report2.run_dir);

    // config_hash must be identical (deterministic placeholder).
    assert_eq!(
        manifest1["config_hash"].as_str(),
        manifest2["config_hash"].as_str(),
        "config_hash must match across runs"
    );

    // engine_id and mode must match.
    assert_eq!(
        manifest1["engine_id"].as_str(),
        manifest2["engine_id"].as_str(),
        "engine_id must match"
    );
    assert_eq!(
        manifest1["mode"].as_str(),
        manifest2["mode"].as_str(),
        "mode must match"
    );

    // git_hash must match.
    assert_eq!(
        manifest1["git_hash"].as_str(),
        manifest2["git_hash"].as_str(),
        "git_hash must match"
    );

    // host_fingerprint must match.
    assert_eq!(
        manifest1["host_fingerprint"].as_str(),
        manifest2["host_fingerprint"].as_str(),
        "host_fingerprint must match"
    );

    Ok(())
}

#[test]
fn determinism_artifact_structure_matches_across_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    let manifest1 = parse_manifest(&report1.run_dir);
    let manifest2 = parse_manifest(&report2.run_dir);

    // Artifact filenames must be identical.
    assert_eq!(
        manifest1["artifacts"], manifest2["artifacts"],
        "artifact filenames must match across runs"
    );

    // schema_version must match.
    assert_eq!(
        manifest1["schema_version"], manifest2["schema_version"],
        "schema_version must match"
    );

    Ok(())
}

#[test]
fn determinism_audit_hash_chain_valid_for_both_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    // Verify hash chain integrity for run 1.
    let audit_path1 = report1.run_dir.join("audit.jsonl");
    let result1 = verify_hash_chain(&audit_path1)?;
    match &result1 {
        VerifyResult::Valid { lines } => {
            assert!(*lines > 0, "run 1 audit chain should have >0 lines");
        }
        VerifyResult::Broken { line, reason } => {
            panic!("run 1 audit hash chain broken at line {line}: {reason}");
        }
    }

    // Verify hash chain integrity for run 2.
    let audit_path2 = report2.run_dir.join("audit.jsonl");
    let result2 = verify_hash_chain(&audit_path2)?;
    match &result2 {
        VerifyResult::Valid { lines } => {
            assert!(*lines > 0, "run 2 audit chain should have >0 lines");
        }
        VerifyResult::Broken { line, reason } => {
            panic!("run 2 audit hash chain broken at line {line}: {reason}");
        }
    }

    // Both chains must have the same number of events.
    if let (VerifyResult::Valid { lines: l1 }, VerifyResult::Valid { lines: l2 }) =
        (&result1, &result2)
    {
        assert_eq!(
            l1, l2,
            "both runs should produce the same number of audit events"
        );
    }

    Ok(())
}

#[test]
fn determinism_fills_and_equity_curve_match_across_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    // Fill counts must match.
    assert_eq!(
        report1.fills_count, report2.fills_count,
        "fills_count must match across runs"
    );
    assert_eq!(
        report1.broker_acks, report2.broker_acks,
        "broker_acks must match across runs"
    );
    assert_eq!(
        report1.broker_fills, report2.broker_fills,
        "broker_fills must match across runs"
    );

    // Equity curve must match point-for-point.
    assert_eq!(
        report1.equity_curve.len(),
        report2.equity_curve.len(),
        "equity curve lengths must match"
    );
    for (i, (ec1, ec2)) in report1
        .equity_curve
        .iter()
        .zip(report2.equity_curve.iter())
        .enumerate()
    {
        assert_eq!(
            ec1.0, ec2.0,
            "equity curve timestamp mismatch at index {i}"
        );
        assert_eq!(
            ec1.1, ec2.1,
            "equity curve value mismatch at index {i}"
        );
    }

    // Bars processed must match.
    assert_eq!(
        report1.bars_processed, report2.bars_processed,
        "bars_processed must match"
    );

    Ok(())
}

#[test]
fn determinism_broker_event_payloads_match_across_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    let events1 = parse_audit_events(&report1.run_dir);
    let events2 = parse_audit_events(&report2.run_dir);

    // Extract deterministic broker event signatures (event_type, symbol, side, qty).
    let sigs1 = extract_broker_event_signatures(&events1);
    let sigs2 = extract_broker_event_signatures(&events2);

    assert_eq!(
        sigs1.len(),
        sigs2.len(),
        "broker event count must match across runs"
    );

    for (i, (s1, s2)) in sigs1.iter().zip(sigs2.iter()).enumerate() {
        assert_eq!(
            s1, s2,
            "broker event signature mismatch at index {i}: run1={s1:?} vs run2={s2:?}"
        );
    }

    Ok(())
}

#[test]
fn determinism_audit_event_topics_match_across_runs() -> Result<()> {
    let tmp1 = tempdir()?;
    let tmp2 = tempdir()?;

    let report1 = run_scenario(tmp1.path())?;
    let report2 = run_scenario(tmp2.path())?;

    let events1 = parse_audit_events(&report1.run_dir);
    let events2 = parse_audit_events(&report2.run_dir);

    assert_eq!(
        events1.len(),
        events2.len(),
        "total audit event count must match across runs"
    );

    // Compare topic + event_type sequence (deterministic).
    let topics1: Vec<(String, String)> = events1
        .iter()
        .map(|ev| {
            (
                ev["topic"].as_str().unwrap_or("").to_string(),
                ev["event_type"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    let topics2: Vec<(String, String)> = events2
        .iter()
        .map(|ev| {
            (
                ev["topic"].as_str().unwrap_or("").to_string(),
                ev["event_type"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    for (i, (t1, t2)) in topics1.iter().zip(topics2.iter()).enumerate() {
        assert_eq!(
            t1, t2,
            "audit event topic/event_type mismatch at index {i}: run1={t1:?} vs run2={t2:?}"
        );
    }

    Ok(())
}
