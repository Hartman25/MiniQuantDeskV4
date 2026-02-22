//! Orchestrator "integrity disarm" scenario (current-state contract).
//!
//! The full integrity engine is NOT wired into `mqk-testkit::Orchestrator` yet.
//! So instead of asserting a DISARM path, these tests pin the *present*
//! behavior that must remain deterministic:
//!   - time gaps do not crash the orchestrator
//!   - the report reflects the last processed bar
//!   - `max_bars` still caps processing even across gaps

use anyhow::Result;
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorRunMeta};
use uuid::Uuid;

fn bar(symbol: &str, day_id: u32, end_ts: u64, close_micros: i64) -> OrchestratorBar {
    OrchestratorBar {
        symbol: symbol.to_string(),
        day_id,
        end_ts,
        open_micros: close_micros,
        high_micros: close_micros,
        low_micros: close_micros,
        close_micros,
        volume: 1,
    }
}

#[test]
fn large_time_gap_does_not_crash_and_last_bar_wins() -> Result<()> {
    let bars = vec![
        bar("SPY", 20250101, 1_700_000_000, 100_000_000),
        bar("SPY", 20250101, 1_700_000_060, 100_100_000),
        // big jump forward
        bar("SPY", 20250101, 1_700_010_000, 100_200_000),
        bar("SPY", 20250101, 1_700_010_060, 100_300_000),
    ];

    let cfg = OrchestratorConfig::test_defaults();
    let meta = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);

    let report = orch.run(&bars)?;

    assert_eq!(report.symbol, "SPY");
    assert_eq!(report.bars_seen, 4);
    assert_eq!(report.last_end_ts, Some(1_700_010_060));
    assert_eq!(report.last_close_micros, Some(100_300_000));

    Ok(())
}

#[test]
fn max_bars_cap_applies_even_when_stream_has_gaps() -> Result<()> {
    // 1 bar, then huge gap, then many bars.
    let mut bars = vec![bar("SPY", 20250101, 1_700_000_000, 100_000_000)];
    bars.push(bar("SPY", 20250101, 1_700_100_000, 101_000_000));
    for i in 0..20u64 {
        bars.push(bar(
            "SPY",
            20250101,
            1_700_100_060 + i * 60,
            101_000_000 + (i as i64) * 10_000,
        ));
    }

    let mut cfg = OrchestratorConfig::test_defaults();
    cfg.max_bars = 5;

    let meta = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);
    let report = orch.run(&bars)?;

    assert_eq!(report.bars_seen, 5);
    // last bar processed is the 5th item in the vector
    assert_eq!(report.last_end_ts, Some(bars[4].end_ts));
    assert_eq!(report.last_close_micros, Some(bars[4].close_micros));

    Ok(())
}
