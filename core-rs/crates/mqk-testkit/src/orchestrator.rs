use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Used by tests/cli as a stable knob. Keep it even if unused right now.
    pub timeframe_secs: i64,

    /// Max bars remembered in the report
    pub max_bars: usize,
}

impl OrchestratorConfig {
    pub fn test_defaults() -> Self {
        Self {
            timeframe_secs: 60,
            max_bars: 512,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorRunMeta {
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorBar {
    pub symbol: String,
    pub day_id: u32,
    pub end_ts: u64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrchestratorReport {
    pub run_id: Uuid,
    pub symbol: String,
    pub bars_seen: usize,
    pub last_end_ts: Option<u64>,
    pub last_close_micros: Option<i64>,
}

pub struct Orchestrator {
    cfg: OrchestratorConfig,
    meta: OrchestratorRunMeta,
    symbol: Option<String>,
    bars_seen: usize,
    last_end_ts: Option<u64>,
    last_close_micros: Option<i64>,
}

impl Orchestrator {
    pub fn new_with_meta(cfg: OrchestratorConfig, meta: OrchestratorRunMeta) -> Self {
        Self {
            cfg,
            meta,
            symbol: None,
            bars_seen: 0,
            last_end_ts: None,
            last_close_micros: None,
        }
    }

    /// Drive orchestrator with a bar stream. For now this just tracks the stream deterministically.
    /// Later we’ll wire strategy/risk/execution/integrity here.
    pub fn run(&mut self, bars: &[OrchestratorBar]) -> Result<OrchestratorReport> {
        for b in bars.iter().take(self.cfg.max_bars) {
            self.symbol = Some(b.symbol.clone());
            self.bars_seen += 1;
            self.last_end_ts = Some(b.end_ts);
            self.last_close_micros = Some(b.close_micros);
        }

        Ok(OrchestratorReport {
            run_id: self.meta.run_id,
            symbol: self.symbol.clone().unwrap_or_default(),
            bars_seen: self.bars_seen,
            last_end_ts: self.last_end_ts,
            last_close_micros: self.last_close_micros,
        })
    }
}
