//! PATCH 23: Orchestrator end-to-end test.
//!
//! Asserts:
//!   - Orchestrator runs through N bars without panic.
//!   - exports/<run_id>/manifest.json exists.
//!   - Audit log exists and has >0 records.

use anyhow::Result;
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig};
use std::fs;
use tempfile::tempdir;

/// Simple strategy: buy 10 shares of SPY on bar 1, hold forever.
struct BuyAndHoldStrategy {
    bought: bool,
}

impl BuyAndHoldStrategy {
    fn new() -> Self {
        Self { bought: false }
    }
}

impl Strategy for BuyAndHoldStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyAndHold", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        if !self.bought {
            self.bought = true;
            StrategyOutput::new(vec![TargetPosition::new("SPY", 10)])
        } else {
            // Maintain position explicitly.
            StrategyOutput::new(vec![TargetPosition::new("SPY", 10)])
        }
    }
}

fn make_bars(n: usize) -> Vec<OrchestratorBar> {
    (0..n)
        .map(|i| {
            let ts = 1_700_000_000 + (i as i64) * 60;
            let price = 100_000_000 + (i as i64) * 100_000; // starts at 100, goes up
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

#[test]
fn orchestrator_runs_through_n_bars_without_panic() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(10);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyAndHoldStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    // Ran all 10 bars.
    assert_eq!(report.bars_processed, 10);
    // At least one fill happened (the initial buy).
    assert!(report.fills_count > 0, "expected at least one fill");
    // Broker produced matching events.
    assert!(report.broker_acks > 0, "expected broker acks");
    assert_eq!(report.broker_acks, report.broker_fills);
    // Not halted or blocked.
    assert!(!report.halted);
    assert!(!report.execution_blocked);

    Ok(())
}

#[test]
fn orchestrator_creates_manifest_json() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyAndHoldStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    // manifest.json must exist under exports/<run_id>/
    let manifest_path = report.run_dir.join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json should exist");

    // Parse and verify run_id.
    let raw = fs::read_to_string(&manifest_path)?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    assert_eq!(v["run_id"].as_str().unwrap(), report.run_id.to_string());
    assert_eq!(v["engine_id"].as_str().unwrap(), "ORCH_MVP");
    assert_eq!(v["mode"].as_str().unwrap(), "PAPER");

    Ok(())
}

#[test]
fn orchestrator_writes_audit_log_with_events() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyAndHoldStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    // audit.jsonl must exist and have >0 records.
    let audit_path = report.run_dir.join("audit.jsonl");
    assert!(audit_path.exists(), "audit.jsonl should exist");

    let content = fs::read_to_string(&audit_path)?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "audit log should have >0 records");
    assert!(report.audit_events > 0, "report should track audit events");

    // Each line should parse as valid JSON.
    for (i, line) in lines.iter().enumerate() {
        let _v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("audit line {} invalid JSON: {}", i, e));
    }

    // Should have at least run_start and run_end events.
    assert!(lines.len() >= 2, "need at least run_start + run_end events");

    Ok(())
}

#[test]
fn orchestrator_equity_curve_has_entries() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(8);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyAndHoldStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    assert_eq!(
        report.equity_curve.len(),
        8,
        "equity curve should have one entry per bar"
    );

    Ok(())
}
