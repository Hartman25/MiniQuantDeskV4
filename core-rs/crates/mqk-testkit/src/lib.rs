use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mqk_schemas::{Bar, BrokerSnapshot};
use std::fs;

pub fn load_bars_csv(path: &str) -> Result<Vec<Bar>> {
    let mut rdr = csv::Reader::from_path(path).with_context(|| format!("open bars csv: {path}"))?;
    let mut out = Vec::new();

    for rec in rdr.records() {
        let rec = rec?;
        let ts: DateTime<Utc> = rec[0].parse().context("parse ts_close_utc")?;
        let bar = Bar {
            ts_close_utc: ts,
            open: rec[1].to_string(),
            high: rec[2].to_string(),
            low: rec[3].to_string(),
            close: rec[4].to_string(),
            volume: rec[5].to_string(),
        };
        out.push(bar);
    }

    // Minimal structural checks (expand later)
    for w in out.windows(2) {
        if !(w[0].ts_close_utc < w[1].ts_close_utc) {
            anyhow::bail!("bars not strictly increasing");
        }
    }

    Ok(out)
}

pub fn load_broker_snapshot_json(path: &str) -> Result<BrokerSnapshot> {
    let s = fs::read_to_string(path).with_context(|| format!("read snapshot: {path}"))?;
    let snap: BrokerSnapshot = serde_json::from_str(&s).context("parse snapshot json")?;
    Ok(snap)
}

/// Placeholder runner for parity scenarios.
/// In Phase 1, this will call into mqk-backtest/mqk-runtime to:
/// - feed bars
/// - collect outbox/inbox
/// - produce artifacts
///
/// For now this is a stub that sets the shape.
pub struct ScenarioRunResult {
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
    pub audit_jsonl: String,
}

pub fn run_parity_scenario_stub(_bars: &[Bar]) -> Result<ScenarioRunResult> {
    // TODO (PATCH 08): replace with real parity backtest runner.
    Ok(ScenarioRunResult {
        orders_csv: String::new(),
        fills_csv: String::new(),
        equity_curve_csv: String::new(),
        metrics_json: String::new(),
        audit_jsonl: String::new(),
    })
}

mod recovery;

pub use recovery::{recover_outbox_against_broker, FakeBroker, RecoveryReport};

pub mod orchestrator;
pub mod paper_broker;

pub use orchestrator::{Orchestrator, OrchestratorBar, OrchestratorConfig, OrchestratorReport};
pub use paper_broker::PaperBroker as OrchestratorPaperBroker;
