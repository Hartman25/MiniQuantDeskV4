//! PATCH 23: Orchestrator integrity disarm test.
//!
//! Asserts:
//!   - A stale/disarm condition occurs (multi-feed/heartbeat technique from Patch 22).
//!   - After DISARM triggers, no new orders are submitted and no new fills occur.

use anyhow::Result;
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig};
use tempfile::tempdir;

/// Strategy that tries to buy every bar (increasing position).
struct BuyEveryBarStrategy;

impl Strategy for BuyEveryBarStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyEveryBar", 60)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        // Target position grows by 10 each bar.
        let target = (ctx.now_tick as i64) * 10;
        StrategyOutput::new(vec![TargetPosition::new("SPY", target)])
    }
}

/// Make N normal bars spaced 60s apart starting at a given timestamp.
fn make_bars_from(start_ts: i64, n: usize) -> Vec<OrchestratorBar> {
    (0..n)
        .map(|i| {
            let ts = start_ts + (i as i64) * 60;
            let price = 100_000_000 + (i as i64) * 50_000;
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
fn disarm_blocks_execution_after_stale_trigger() -> Result<()> {
    let tmp = tempdir()?;
    let start_ts: i64 = 1_700_000_000;

    // 3 normal bars, then a big time gap, then 3 more bars.
    let mut bars = make_bars_from(start_ts, 3);
    // Large gap: jump 10_000 seconds forward (exceeds threshold of 500).
    let gap_bars = make_bars_from(start_ts + 3 * 60 + 10_000, 3);
    bars.extend(gap_bars);

    let mut config = OrchestratorConfig::test_defaults();
    config.integrity_enabled = true;
    config.integrity_stale_threshold_ticks = 500; // 500 seconds
    config.integrity_gap_tolerance_bars = 1000; // high tolerance so gap doesn't halt
    config.integrity_enforce_feed_disagreement = false;

    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyEveryBarStrategy))?;

    // Seed a heartbeat feed at the first bar's timestamp.
    // This feed will go stale when the orchestrator feed jumps forward.
    orch.seed_integrity_feed("heartbeat", start_ts as u64);

    let report = orch.run(&bars, tmp.path())?;

    // Execution should be blocked after the stale trigger.
    assert!(
        report.execution_blocked,
        "integrity should have triggered DISARM"
    );

    // Fills should have occurred for the first 3 bars only (before the gap).
    // After gap, execution_blocked prevents new fills.
    assert!(
        report.fills_count > 0,
        "should have fills from bars before disarm"
    );

    // The broker should have the same count of fills as portfolio fills.
    assert_eq!(report.broker_fills, report.fills_count);

    Ok(())
}

#[test]
fn disarm_no_fills_after_trigger() -> Result<()> {
    let tmp = tempdir()?;
    let start_ts: i64 = 1_700_000_000;

    // Make bars with a gap big enough to trigger stale on bar 4.
    // Bars 1-3: normal spacing (60s apart).
    // Bar 4+: 10000s gap from bar 3.
    let mut bars = make_bars_from(start_ts, 3);
    let gap_bars = make_bars_from(start_ts + 3 * 60 + 10_000, 5);
    bars.extend(gap_bars);

    let mut config = OrchestratorConfig::test_defaults();
    config.integrity_enabled = true;
    config.integrity_stale_threshold_ticks = 500;
    config.integrity_gap_tolerance_bars = 1000;
    config.integrity_enforce_feed_disagreement = false;

    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyEveryBarStrategy))?;
    orch.seed_integrity_feed("heartbeat", start_ts as u64);

    let report = orch.run(&bars, tmp.path())?;

    // Count fills from broker — should match fills_count.
    let broker = orch.broker();
    let fills_before_disarm = broker.fill_count();
    assert_eq!(fills_before_disarm, report.fills_count);

    // The fills should correspond to the first 3 bars only.
    // BuyEveryBarStrategy targets 10, 20, 30 at bars 1, 2, 3 — that's 3 buy intents.
    assert_eq!(
        report.fills_count, 3,
        "should have exactly 3 fills (one per bar before disarm)"
    );

    // Execution was blocked.
    assert!(report.execution_blocked);

    // All 8 bars were still processed (strategy still ran, equity recorded).
    assert_eq!(report.bars_processed, 8);

    Ok(())
}

#[test]
fn no_disarm_when_integrity_disabled() -> Result<()> {
    let tmp = tempdir()?;
    let start_ts: i64 = 1_700_000_000;

    // Same gap scenario but integrity disabled.
    let mut bars = make_bars_from(start_ts, 3);
    let gap_bars = make_bars_from(start_ts + 3 * 60 + 10_000, 3);
    bars.extend(gap_bars);

    let config = OrchestratorConfig::test_defaults(); // integrity_enabled = false

    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyEveryBarStrategy))?;

    let report = orch.run(&bars, tmp.path())?;

    // No execution blocking — all 6 bars should produce fills.
    assert!(!report.execution_blocked);
    assert_eq!(report.fills_count, 6, "every bar should produce a fill");
    assert_eq!(report.bars_processed, 6);

    Ok(())
}

#[test]
fn disarm_audit_log_records_integrity_event() -> Result<()> {
    let tmp = tempdir()?;
    let start_ts: i64 = 1_700_000_000;

    let mut bars = make_bars_from(start_ts, 3);
    let gap_bars = make_bars_from(start_ts + 3 * 60 + 10_000, 2);
    bars.extend(gap_bars);

    let mut config = OrchestratorConfig::test_defaults();
    config.integrity_enabled = true;
    config.integrity_stale_threshold_ticks = 500;
    config.integrity_gap_tolerance_bars = 1000;
    config.integrity_enforce_feed_disagreement = false;

    let mut orch = Orchestrator::new(config);
    orch.add_strategy(Box::new(BuyEveryBarStrategy))?;
    orch.seed_integrity_feed("heartbeat", start_ts as u64);

    let report = orch.run(&bars, tmp.path())?;

    // Audit log should contain an integrity blocked event.
    let audit_path = report.run_dir.join("audit.jsonl");
    let content = std::fs::read_to_string(&audit_path)?;

    let has_integrity_event = content.lines().any(|line| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            v["topic"].as_str() == Some("integrity")
                && v["event_type"].as_str() == Some("execution_blocked")
        } else {
            false
        }
    });

    assert!(
        has_integrity_event,
        "audit log should contain an integrity execution_blocked event"
    );

    Ok(())
}
