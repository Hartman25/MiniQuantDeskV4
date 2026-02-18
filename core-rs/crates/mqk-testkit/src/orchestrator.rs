//! PATCH 23: Minimum Viable Runtime Orchestrator (offline / deterministic).
//!
//! Composes existing engines into a single loop:
//!   bars → integrity → strategy → execution → paper broker → portfolio → risk → audit/artifacts
//!
//! Runs under one `run_id`, writes real artifacts (manifest.json + audit.jsonl).
//!
//! This orchestrator is intentionally minimal:
//! - No network I/O.
//! - Deterministic fill model (paper broker fills at bar close).
//! - Integrity disarm (PATCH 22) blocks execution end-to-end.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use mqk_artifacts::{init_run_artifacts, InitRunArtifactsArgs};
use mqk_audit::AuditWriter;
use mqk_execution::{targets_to_order_intents, PositionBook, Side as ExecSide};
use mqk_integrity::{
    evaluate_bar as integrity_evaluate_bar, tick_feed, Bar as IntegrityBar, BarKey, FeedId,
    IntegrityAction, IntegrityConfig, IntegrityState, Timeframe as IntegrityTimeframe,
};
use mqk_portfolio::{
    apply_fill, compute_equity_micros, Fill, MarkMap, PortfolioState, Side as PfSide,
};
use mqk_risk::{
    evaluate as risk_evaluate, PdtContext, RequestKind, RiskAction, RiskConfig, RiskInput,
    RiskState,
};
use mqk_strategy::{
    BarStub, RecentBarsWindow, ShadowMode, Strategy, StrategyContext, StrategyHost,
};

use crate::paper_broker::PaperBroker;

/// Input bar for the orchestrator (mirrors BacktestBar but owned by testkit).
#[derive(Clone, Debug)]
pub struct OrchestratorBar {
    pub symbol: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
    pub day_id: u32,
}

/// Configuration for the orchestrator run.
#[derive(Clone, Debug)]
pub struct OrchestratorConfig {
    /// Bar timeframe in seconds.
    pub timeframe_secs: i64,
    /// Initial cash in micros.
    pub initial_cash_micros: i64,
    /// Maximum recent bars for strategy context.
    pub bar_history_len: usize,
    /// Enable integrity checking.
    pub integrity_enabled: bool,
    /// Stale threshold in ticks.
    pub integrity_stale_threshold_ticks: u64,
    /// Gap tolerance bars.
    pub integrity_gap_tolerance_bars: u32,
    /// Enforce feed disagreement.
    pub integrity_enforce_feed_disagreement: bool,
    /// Enable hash chain on audit log.
    pub audit_hash_chain: bool,
}

impl OrchestratorConfig {
    pub fn test_defaults() -> Self {
        Self {
            timeframe_secs: 60,
            initial_cash_micros: 100_000_000_000, // 100k
            bar_history_len: 50,
            integrity_enabled: false,
            integrity_stale_threshold_ticks: 0,
            integrity_gap_tolerance_bars: 0,
            integrity_enforce_feed_disagreement: false,
            audit_hash_chain: true,
        }
    }
}

/// Report produced after an orchestrator run.
#[derive(Clone, Debug)]
pub struct OrchestratorReport {
    pub run_id: Uuid,
    pub bars_processed: usize,
    pub fills_count: usize,
    pub broker_acks: usize,
    pub broker_fills: usize,
    pub audit_events: usize,
    pub execution_blocked: bool,
    pub halted: bool,
    pub run_dir: PathBuf,
    pub equity_curve: Vec<(i64, i64)>,
}

/// The orchestrator: composes all crate engines into one offline loop.
pub struct Orchestrator {
    config: OrchestratorConfig,
    run_id: Uuid,
    host: StrategyHost,
    portfolio: PortfolioState,
    risk_config: RiskConfig,
    risk_state: Option<RiskState>,
    integrity_config: IntegrityConfig,
    integrity_state: IntegrityState,
    broker: PaperBroker,
    last_prices: MarkMap,
    fills: Vec<Fill>,
    equity_curve: Vec<(i64, i64)>,
    recent_bars: Vec<BarStub>,
    bar_count: u64,
    halted: bool,
    execution_blocked: bool,
}

impl Orchestrator {
    pub fn new(config: OrchestratorConfig) -> Self {
        let host = StrategyHost::new(ShadowMode::Off);
        let portfolio = PortfolioState::new(config.initial_cash_micros);
        let risk_config = RiskConfig::sane_defaults();
        let integrity_config = IntegrityConfig {
            gap_tolerance_bars: config.integrity_gap_tolerance_bars,
            stale_threshold_ticks: config.integrity_stale_threshold_ticks,
            enforce_feed_disagreement: config.integrity_enforce_feed_disagreement,
        };

        Self {
            config,
            run_id: Uuid::new_v4(),
            host,
            portfolio,
            risk_config,
            risk_state: None,
            integrity_config,
            integrity_state: IntegrityState::new(),
            broker: PaperBroker::new(),
            last_prices: BTreeMap::new(),
            fills: Vec::new(),
            equity_curve: Vec::new(),
            recent_bars: Vec::new(),
            bar_count: 0,
            halted: false,
            execution_blocked: false,
        }
    }

    /// Register a strategy (must be called before run).
    pub fn add_strategy(&mut self, s: Box<dyn Strategy>) -> Result<()> {
        self.host
            .register(s)
            .map_err(|e| anyhow::anyhow!("strategy registration failed: {:?}", e))
    }

    /// Seed an integrity feed (for multi-feed stale detection).
    pub fn seed_integrity_feed(&mut self, feed_name: &str, tick: u64) {
        let feed = FeedId::new(feed_name);
        tick_feed(
            &self.integrity_config,
            &mut self.integrity_state,
            &feed,
            tick,
        );
    }

    /// Returns the run_id (for inspecting artifacts after run).
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }

    /// Whether execution is currently blocked by integrity.
    pub fn is_execution_blocked(&self) -> bool {
        self.execution_blocked
    }

    /// Reference to the integrity state.
    pub fn integrity_state(&self) -> &IntegrityState {
        &self.integrity_state
    }

    /// Reference to the paper broker.
    pub fn broker(&self) -> &PaperBroker {
        &self.broker
    }

    /// Run the full orchestrator loop over the given bars, writing artifacts.
    pub fn run(
        &mut self,
        bars: &[OrchestratorBar],
        exports_root: &Path,
    ) -> Result<OrchestratorReport> {
        // 1. Initialize artifacts (manifest.json + placeholder files).
        let config_hash = format!("{:016x}", 0u64); // deterministic placeholder
        let artifacts = init_run_artifacts(InitRunArtifactsArgs {
            exports_root,
            schema_version: 1,
            run_id: self.run_id,
            engine_id: "ORCH_MVP",
            mode: "PAPER",
            git_hash: "000000",
            config_hash: &config_hash,
            host_fingerprint: "test|orchestrator|mvp",
        })
        .context("init_run_artifacts failed")?;

        // 2. Create audit writer in the run directory.
        let audit_path = artifacts.run_dir.join("audit.jsonl");
        let mut audit = AuditWriter::new(&audit_path, self.config.audit_hash_chain)
            .context("create audit writer failed")?;
        let mut audit_event_count = 0usize;

        // Write run_start event.
        audit.append(
            self.run_id,
            "lifecycle",
            "run_start",
            serde_json::json!({
                "engine_id": "ORCH_MVP",
                "mode": "PAPER",
                "bar_count": bars.len(),
            }),
        )?;
        audit_event_count += 1;

        // 3. Main loop: bars → integrity → strategy → execution → broker → portfolio → risk.
        let mut bars_processed = 0usize;

        for bar in bars {
            if self.halted {
                break;
            }

            // --- Integrity gate ---
            if self.config.integrity_enabled {
                let feed = FeedId::new("orchestrator");
                let now_tick = bar.end_ts as u64;
                let int_bar = IntegrityBar::new(
                    BarKey::new(
                        bar.symbol.clone(),
                        IntegrityTimeframe::secs(self.config.timeframe_secs),
                        bar.end_ts,
                    ),
                    bar.is_complete,
                    bar.close_micros,
                    bar.volume,
                );
                let decision = integrity_evaluate_bar(
                    &self.integrity_config,
                    &mut self.integrity_state,
                    &feed,
                    now_tick,
                    &int_bar,
                );
                match decision.action {
                    IntegrityAction::Disarm | IntegrityAction::Halt | IntegrityAction::Reject => {
                        if !self.execution_blocked {
                            // Log the disarm event.
                            audit.append(
                                self.run_id,
                                "integrity",
                                "execution_blocked",
                                serde_json::json!({
                                    "action": format!("{:?}", decision.action),
                                    "reason": format!("{:?}", decision.reason),
                                    "bar_end_ts": bar.end_ts,
                                }),
                            )?;
                            audit_event_count += 1;
                        }
                        self.execution_blocked = true;
                    }
                    IntegrityAction::Allow => {}
                }
            }

            // --- Update marks ---
            self.last_prices
                .insert(bar.symbol.clone(), bar.close_micros);

            // --- Lazy-init risk state ---
            if self.risk_state.is_none() {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.risk_state = Some(RiskState::new(bar.day_id, equity, 0));
            }

            // --- Strategy ---
            self.bar_count += 1;
            let stub = BarStub::new(bar.end_ts, bar.is_complete, bar.close_micros, bar.volume);
            self.recent_bars.push(stub);
            if self.recent_bars.len() > self.config.bar_history_len {
                let start = self.recent_bars.len() - self.config.bar_history_len;
                self.recent_bars = self.recent_bars.split_off(start);
            }

            let recent =
                RecentBarsWindow::new(self.config.bar_history_len, self.recent_bars.clone());
            let ctx = StrategyContext::new(self.config.timeframe_secs, self.bar_count, recent);

            let bar_result = self
                .host
                .on_bar(&ctx)
                .map_err(|e| anyhow::anyhow!("strategy on_bar failed: {:?}", e))?;

            // --- Shadow mode check ---
            if !bar_result.intents.should_execute() {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.equity_curve.push((bar.end_ts, equity));
                bars_processed += 1;
                continue;
            }

            // --- Integrity disarm gate ---
            if self.execution_blocked {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.equity_curve.push((bar.end_ts, equity));
                bars_processed += 1;
                continue;
            }

            // --- Execution: convert targets to order intents ---
            let position_book = self.build_position_book();
            let exec_decision =
                targets_to_order_intents(&position_book, &bar_result.intents.output);

            // --- Process each intent through risk → broker → portfolio ---
            for intent in &exec_decision.intents {
                if self.halted {
                    break;
                }

                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );

                let is_risk_reducing = self.is_intent_risk_reducing(intent);

                let risk_input = RiskInput {
                    day_id: bar.day_id,
                    equity_micros: equity,
                    reject_window_id: 0,
                    request: RequestKind::NewOrder,
                    is_risk_reducing,
                    pdt: PdtContext::ok(),
                    kill_switch: None,
                };

                let risk_state = self.risk_state.as_mut().unwrap();
                let risk_decision = risk_evaluate(&self.risk_config, risk_state, &risk_input);

                match risk_decision.action {
                    RiskAction::Allow => {
                        let side_str = match intent.side {
                            ExecSide::Buy => "BUY",
                            ExecSide::Sell => "SELL",
                        };

                        // Paper broker: fill at bar close.
                        let (ack, broker_fill) = self.broker.submit_order(
                            &intent.symbol,
                            side_str,
                            intent.qty,
                            bar.close_micros,
                        );

                        // Audit the ack + fill.
                        audit.append(
                            self.run_id,
                            "broker",
                            "order_ack",
                            serde_json::to_value(&ack)?,
                        )?;
                        audit_event_count += 1;

                        audit.append(
                            self.run_id,
                            "broker",
                            "fill",
                            serde_json::to_value(&broker_fill)?,
                        )?;
                        audit_event_count += 1;

                        // Apply fill to portfolio.
                        let pf_side = match intent.side {
                            ExecSide::Buy => PfSide::Buy,
                            ExecSide::Sell => PfSide::Sell,
                        };
                        let fill = Fill::new(
                            intent.symbol.clone(),
                            pf_side,
                            intent.qty,
                            bar.close_micros,
                            0,
                        );
                        apply_fill(&mut self.portfolio, &fill);
                        self.fills.push(fill);
                    }
                    RiskAction::Reject => {
                        // Rejected by risk engine — skip.
                    }
                    RiskAction::Halt => {
                        self.halted = true;
                    }
                    RiskAction::FlattenAndHalt => {
                        self.halted = true;
                    }
                }
            }

            // --- Equity curve point ---
            let equity = compute_equity_micros(
                self.portfolio.cash_micros,
                &self.portfolio.positions,
                &self.last_prices,
            );
            self.equity_curve.push((bar.end_ts, equity));
            bars_processed += 1;
        }

        // 4. Write run_end event.
        audit.append(
            self.run_id,
            "lifecycle",
            "run_end",
            serde_json::json!({
                "bars_processed": bars_processed,
                "fills": self.fills.len(),
                "halted": self.halted,
                "execution_blocked": self.execution_blocked,
            }),
        )?;
        audit_event_count += 1;

        Ok(OrchestratorReport {
            run_id: self.run_id,
            bars_processed,
            fills_count: self.fills.len(),
            broker_acks: self.broker.ack_count(),
            broker_fills: self.broker.fill_count(),
            audit_events: audit_event_count,
            execution_blocked: self.execution_blocked,
            halted: self.halted,
            run_dir: artifacts.run_dir,
            equity_curve: self.equity_curve.clone(),
        })
    }

    fn build_position_book(&self) -> PositionBook {
        let mut book = PositionBook::new();
        for (sym, pos) in &self.portfolio.positions {
            let qty = pos.qty_signed();
            if qty != 0 {
                book.insert(sym.clone(), qty);
            }
        }
        book
    }

    fn is_intent_risk_reducing(&self, intent: &mqk_execution::OrderIntent) -> bool {
        let current_qty = self
            .portfolio
            .positions
            .get(&intent.symbol)
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        match intent.side {
            ExecSide::Buy => current_qty < 0,
            ExecSide::Sell => current_qty > 0,
        }
    }
}
