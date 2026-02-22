//! Replay determinism scenario (current-state contract).
//!
//! The orchestrator currently produces only an in-memory `OrchestratorReport`.
//! This test pins determinism at that boundary:
//!   - same bars + same meta => identical reports
//!   - same bars + different run_id => only run_id differs

use anyhow::Result;
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorRunMeta};
use uuid::Uuid;

fn make_bars(n: usize) -> Vec<OrchestratorBar> {
    (0..n)
        .map(|i| {
            let ts = 1_700_000_000_u64 + (i as u64) * 60;
            let price = 100_000_000_i64 + (i as i64) * 123_456;
            OrchestratorBar {
                symbol: "SPY".to_string(),
                day_id: 20250101,
                end_ts: ts,
                open_micros: price,
                high_micros: price,
                low_micros: price,
                close_micros: price,
                volume: 1000,
            }
        })
        .collect()
}

fn run_once(run_id: Uuid, bars: &[OrchestratorBar]) -> Result<mqk_testkit::OrchestratorReport> {
    let cfg = OrchestratorConfig::test_defaults();
    let meta = OrchestratorRunMeta {
        run_id,
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);
    orch.run(bars)
}

#[test]
fn same_inputs_same_report() -> Result<()> {
    let bars = make_bars(12);
    let run_id = Uuid::nil();

    let r1 = run_once(run_id, &bars)?;
    let r2 = run_once(run_id, &bars)?;

    // Full equality is safe because we fixed run_id.
    assert_eq!(r1.run_id, r2.run_id);
    assert_eq!(r1.symbol, r2.symbol);
    assert_eq!(r1.bars_seen, r2.bars_seen);
    assert_eq!(r1.last_end_ts, r2.last_end_ts);
    assert_eq!(r1.last_close_micros, r2.last_close_micros);

    Ok(())
}

#[test]
fn different_run_id_only_changes_run_id_field() -> Result<()> {
    let bars = make_bars(12);

    let r1 = run_once(Uuid::nil(), &bars)?;
    let r2 = run_once(Uuid::new_v4(), &bars)?;

    assert_ne!(r1.run_id, r2.run_id);
    assert_eq!(r1.symbol, r2.symbol);
    assert_eq!(r1.bars_seen, r2.bars_seen);
    assert_eq!(r1.last_end_ts, r2.last_end_ts);
    assert_eq!(r1.last_close_micros, r2.last_close_micros);

    Ok(())
}
