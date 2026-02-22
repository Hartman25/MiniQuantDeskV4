//! "Entry + protective stop" scenario (current-state contract).
//!
//! The strategy/risk/execution stack is not wired into the testkit orchestrator yet.
//! So this file now asserts the deterministic *data-plane* behavior that will be
//! required once those layers are connected:
//!   - the orchestrator sees a stream of bars
//!   - it always reports the last processed bar (ts + close)
//!   - it preserves symbol identity for the stream

use anyhow::Result;
use mqk_testkit::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorRunMeta};
use uuid::Uuid;

fn make_bars(symbol: &str) -> Vec<OrchestratorBar> {
    let base_ts = 1_700_000_000_u64;
    let day_id = 20250101_u32;
    let closes = [
        100_000_000_i64,
        99_900_000,
        99_800_000,
        100_050_000,
        100_100_000,
    ];

    closes
        .iter()
        .enumerate()
        .map(|(i, close)| OrchestratorBar {
            symbol: symbol.to_string(),
            day_id,
            end_ts: base_ts + (i as u64) * 60,
            open_micros: *close,
            high_micros: *close,
            low_micros: *close,
            close_micros: *close,
            volume: 1000,
        })
        .collect()
}

#[test]
fn reports_last_close_and_timestamp() -> Result<()> {
    let bars = make_bars("SPY");

    let cfg = OrchestratorConfig::test_defaults();
    let meta = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let mut orch = Orchestrator::new_with_meta(cfg, meta);

    let report = orch.run(&bars)?;

    assert_eq!(report.symbol, "SPY");
    assert_eq!(report.bars_seen, 5);
    assert_eq!(report.last_end_ts, Some(1_700_000_000 + 4 * 60));
    assert_eq!(report.last_close_micros, Some(100_100_000));

    Ok(())
}

#[test]
fn symbol_is_taken_from_stream_and_is_deterministic() -> Result<()> {
    let bars_a = make_bars("AAPL");
    let bars_b = make_bars("MSFT");

    let cfg = OrchestratorConfig::test_defaults();
    let meta_a = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };
    let meta_b = OrchestratorRunMeta {
        run_id: Uuid::nil(),
        engine_id: "ORCH_MVP".to_string(),
        mode: "TEST".to_string(),
    };

    let mut orch_a = Orchestrator::new_with_meta(cfg.clone(), meta_a);
    let mut orch_b = Orchestrator::new_with_meta(cfg, meta_b);

    let report_a = orch_a.run(&bars_a)?;
    let report_b = orch_b.run(&bars_b)?;

    assert_eq!(report_a.symbol, "AAPL");
    assert_eq!(report_b.symbol, "MSFT");

    Ok(())
}
