//! Orchestrator end-to-end smoke tests.
//!
//! NOTE: The `mqk-testkit` orchestrator is intentionally minimal right now.
//! These tests assert only what the current contract guarantees:
//!   - It deterministically tracks a bar stream
//!   - It caps processing by `max_bars`
//!   - It reports last seen timestamp + close

use anyhow::Result;
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorRunMeta};
use uuid::Uuid;

fn make_bars(n: usize, start_ts: u64) -> Vec<OrchestratorBar> {
    (0..n)
        .map(|i| {
            let ts = start_ts + (i as u64) * 60;
            let price = 100_000_000_i64 + (i as i64) * 100_000; // 100.0 -> up
            OrchestratorBar {
                symbol: "SPY".to_string(),
                day_id: 20250101,
                end_ts: ts,
                open_micros: price - 50_000,
                high_micros: price + 100_000,
                low_micros: price - 100_000,
                close_micros: price,
                volume: 1000,
            }
        })
        .collect()
}

#[test]
fn orchestrator_tracks_last_bar_deterministically() -> Result<()> {
    let bars = make_bars(10, 1_700_000_000);

    let cfg = OrchestratorConfig::test_defaults();
    let meta = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);

    let report = orch.run(&bars)?;

    assert_eq!(report.symbol, "SPY");
    assert_eq!(report.bars_seen, 10);
    assert_eq!(report.last_end_ts, Some(1_700_000_000 + 9 * 60));
    assert_eq!(report.last_close_micros, Some(100_000_000 + 9 * 100_000));

    Ok(())
}

#[test]
fn orchestrator_caps_processing_by_max_bars() -> Result<()> {
    let bars = make_bars(50, 1_700_010_000);

    let mut cfg = OrchestratorConfig::test_defaults();
    cfg.max_bars = 7;

    let meta = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);

    let report = orch.run(&bars)?;

    assert_eq!(report.bars_seen, 7);
    assert_eq!(report.last_end_ts, Some(1_700_010_000 + 6 * 60));
    assert_eq!(report.last_close_micros, Some(100_000_000 + 6 * 100_000));

    Ok(())
}
