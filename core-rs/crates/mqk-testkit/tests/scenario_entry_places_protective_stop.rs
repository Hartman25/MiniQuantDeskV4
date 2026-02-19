//! PATCH 24: Entry + protective-stop parity scenario.
//!
//! Asserts:
//!   - Orchestrator runs a strategy that enters a long position (BUY)
//!     and then places a protective stop (SELL) on the next bar.
//!   - manifest.json + audit.jsonl exist in the run directory.
//!   - Audit log contains broker events: entry BUY ack+fill BEFORE
//!     protective-stop SELL ack+fill.
//!   - Event field assertions use parsed JSON (topic/event_type/payload),
//!     not string matching.

use anyhow::Result;
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig};
use std::fs;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Strategy: buy bar 1 (entry), reduce to 0 bar 2 (protective stop sell).
// ---------------------------------------------------------------------------

struct EntryThenStopStrategy {
    bar_num: u64,
}

impl EntryThenStopStrategy {
    fn new() -> Self {
        Self { bar_num: 0 }
    }
}

impl Strategy for EntryThenStopStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("EntryThenStop", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_num += 1;
        match self.bar_num {
            1 => {
                // Bar 1: enter long 10 shares of SPY.
                StrategyOutput::new(vec![TargetPosition::new("SPY", 10)])
            }
            2 => {
                // Bar 2: protective stop — flatten to 0.
                StrategyOutput::new(vec![TargetPosition::new("SPY", 0)])
            }
            _ => {
                // Bars 3+: stay flat.
                StrategyOutput::new(vec![TargetPosition::new("SPY", 0)])
            }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn entry_places_protective_stop_manifest_and_audit_exist() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(EntryThenStopStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    // manifest.json must exist.
    let manifest_path = report.run_dir.join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json should exist");

    // audit.jsonl must exist and have >0 lines.
    let audit_path = report.run_dir.join("audit.jsonl");
    assert!(audit_path.exists(), "audit.jsonl should exist");

    let events = parse_audit_events(&report.run_dir);
    assert!(
        !events.is_empty(),
        "audit log should have at least one event"
    );

    Ok(())
}

#[test]
fn entry_buy_ack_and_fill_present_in_audit() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(EntryThenStopStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;
    let events = parse_audit_events(&report.run_dir);

    // Find broker/order_ack with side == "BUY".
    let buy_ack = events.iter().find(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("order_ack")
            && ev["payload"]["side"].as_str() == Some("BUY")
    });
    assert!(
        buy_ack.is_some(),
        "audit should contain a broker/order_ack for BUY entry"
    );

    // Find broker/fill with side == "BUY".
    let buy_fill = events.iter().find(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("fill")
            && ev["payload"]["side"].as_str() == Some("BUY")
    });
    assert!(
        buy_fill.is_some(),
        "audit should contain a broker/fill for BUY entry"
    );

    // Verify the BUY fill payload fields.
    let fill_payload = &buy_fill.unwrap()["payload"];
    assert_eq!(
        fill_payload["symbol"].as_str(),
        Some("SPY"),
        "fill symbol should be SPY"
    );
    assert_eq!(
        fill_payload["qty"].as_i64(),
        Some(10),
        "fill qty should be 10"
    );

    Ok(())
}

#[test]
fn protective_stop_sell_ack_and_fill_present_in_audit() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(EntryThenStopStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;
    let events = parse_audit_events(&report.run_dir);

    // Find broker/order_ack with side == "SELL" (protective stop).
    let sell_ack = events.iter().find(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("order_ack")
            && ev["payload"]["side"].as_str() == Some("SELL")
    });
    assert!(
        sell_ack.is_some(),
        "audit should contain a broker/order_ack for SELL (protective stop)"
    );

    // Find broker/fill with side == "SELL".
    let sell_fill = events.iter().find(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("fill")
            && ev["payload"]["side"].as_str() == Some("SELL")
    });
    assert!(
        sell_fill.is_some(),
        "audit should contain a broker/fill for SELL (protective stop)"
    );

    // Verify the SELL fill payload fields.
    let fill_payload = &sell_fill.unwrap()["payload"];
    assert_eq!(
        fill_payload["symbol"].as_str(),
        Some("SPY"),
        "sell fill symbol should be SPY"
    );
    assert_eq!(
        fill_payload["qty"].as_i64(),
        Some(10),
        "sell fill qty should be 10 (flatten from 10 to 0)"
    );

    Ok(())
}

#[test]
fn entry_buy_appears_before_protective_stop_sell() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(EntryThenStopStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;
    let events = parse_audit_events(&report.run_dir);

    // Find index of first BUY broker/fill.
    let buy_fill_idx = events.iter().position(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("fill")
            && ev["payload"]["side"].as_str() == Some("BUY")
    });

    // Find index of first SELL broker/fill (protective stop).
    let sell_fill_idx = events.iter().position(|ev| {
        ev["topic"].as_str() == Some("broker")
            && ev["event_type"].as_str() == Some("fill")
            && ev["payload"]["side"].as_str() == Some("SELL")
    });

    assert!(buy_fill_idx.is_some(), "must have a BUY fill in audit log");
    assert!(
        sell_fill_idx.is_some(),
        "must have a SELL fill in audit log"
    );

    assert!(
        buy_fill_idx.unwrap() < sell_fill_idx.unwrap(),
        "BUY entry fill (index {}) must appear before SELL protective-stop fill (index {})",
        buy_fill_idx.unwrap(),
        sell_fill_idx.unwrap(),
    );

    Ok(())
}

#[test]
fn entry_and_stop_produces_correct_fill_count() -> Result<()> {
    let tmp = tempdir()?;
    let bars = make_bars(5);

    let config = OrchestratorConfig::test_defaults();
    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(EntryThenStopStrategy::new()))?;

    let report = orch.run(&bars, tmp.path())?;

    // Strategy targets: bar 1 → BUY 10, bar 2 → SELL 10 (flatten),
    // bars 3-5 → target 0 (no change, no fills).
    // Expect exactly 2 fills total.
    assert_eq!(
        report.fills_count, 2,
        "should have exactly 2 fills (entry + protective stop)"
    );
    assert_eq!(report.broker_acks, 2, "should have 2 broker acks");
    assert_eq!(report.broker_fills, 2, "should have 2 broker fills");
    assert_eq!(report.bars_processed, 5, "all 5 bars processed");
    assert!(!report.halted, "should not be halted");

    Ok(())
}
