use std::collections::BTreeMap;

use mqk_execution::{targets_to_order_intents, PositionBook, Side as ExecSide};
use mqk_portfolio::{
    apply_fill, compute_equity_micros, compute_exposure_micros, Fill, MarkMap, PortfolioState,
    Side as PfSide,
};
use mqk_risk::{
    evaluate as risk_evaluate, PdtContext, RequestKind, RiskAction, RiskConfig, RiskInput, RiskState,
};
use mqk_strategy::{
    BarStub, RecentBarsWindow, ShadowMode, Strategy, StrategyContext, StrategyHost, StrategyHostError,
};

use mqk_isolation::enforce_allocation_cap_micros;

use crate::types::{BacktestBar, BacktestConfig, BacktestReport};

/// Backtest error variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BacktestError {
    /// A bar was marked incomplete (anti-lookahead).
    IncompleteBar { symbol: String, end_ts: i64 },
    /// Negative timestamp is invalid.
    NegativeTimestamp { end_ts: i64 },
    /// Strategy host error (forwarded).
    StrategyHost(StrategyHostError),
}

impl core::fmt::Display for BacktestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BacktestError::IncompleteBar { symbol, end_ts } => {
                write!(f, "incomplete bar: {} @ ts={}", symbol, end_ts)
            }
            BacktestError::NegativeTimestamp { end_ts } => {
                write!(f, "negative timestamp: {}", end_ts)
            }
            BacktestError::StrategyHost(e) => write!(f, "strategy host: {:?}", e),
        }
    }
}

/// The backtest engine: event-sourced, deterministic replay.
///
/// Pipeline per bar: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
pub struct BacktestEngine {
    config: BacktestConfig,
    host: StrategyHost,
    portfolio: PortfolioState,
    risk_state: Option<RiskState>,
    risk_config: RiskConfig,
    /// Recent bars for strategy context (bounded window).
    recent_bars: Vec<BarStub>,
    /// Last known price per symbol (for equity/mark computation).
    last_prices: MarkMap,
    /// All fills recorded during the run.
    fills: Vec<Fill>,
    /// Equity curve: (end_ts, equity_micros).
    equity_curve: Vec<(i64, i64)>,
    /// Whether the engine has halted.
    halted: bool,
    /// Reason for halt.
    halt_reason: Option<String>,
    /// Bar counter (deterministic tick).
    bar_count: u64,
}

impl BacktestEngine {
    pub fn new(config: BacktestConfig) -> Self {
        let shadow = if config.shadow_mode {
            ShadowMode::On
        } else {
            ShadowMode::Off
        };
        let host = StrategyHost::new(shadow);
        let portfolio = PortfolioState::new(config.initial_cash_micros);

        let risk_config = RiskConfig {
            daily_loss_limit_micros: config.daily_loss_limit_micros,
            max_drawdown_limit_micros: config.max_drawdown_limit_micros,
            reject_storm_max_rejects_in_window: config.reject_storm_max_rejects,
            pdt_auto_enabled: config.pdt_enabled,
            missing_protective_stop_flattens: config.kill_switch_flattens,
        };

        Self {
            config,
            host,
            portfolio,
            risk_state: None,
            risk_config,
            recent_bars: Vec::new(),
            last_prices: BTreeMap::new(),
            fills: Vec::new(),
            equity_curve: Vec::new(),
            halted: false,
            halt_reason: None,
            bar_count: 0,
        }
    }

    /// Register a strategy. Must be called before run().
    pub fn add_strategy(&mut self, s: Box<dyn Strategy>) -> Result<(), BacktestError> {
        self.host.register(s).map_err(BacktestError::StrategyHost)
    }

    /// Run the backtest on a sequence of bars.
    ///
    /// Event-sourced pipeline per bar:
    /// 1. Validate bar (incomplete => error, negative ts => error)
    /// 2. Update last prices / marks
    /// 3. Feed bar into strategy host
    /// 4. Convert strategy targets to order intents (mqk-execution)
    /// 5. For each intent: allocation-cap check, risk check, apply slippage, create fill, apply to portfolio
    /// 6. Record equity curve point
    /// 7. Handle halt/flatten actions from risk engine
    pub fn run(&mut self, bars: &[BacktestBar]) -> Result<BacktestReport, BacktestError> {
        for bar in bars {
            if self.halted {
                break;
            }

            // 1. Validate bar
            if !bar.is_complete {
                return Err(BacktestError::IncompleteBar {
                    symbol: bar.symbol.clone(),
                    end_ts: bar.end_ts,
                });
            }
            if bar.end_ts < 0 {
                return Err(BacktestError::NegativeTimestamp { end_ts: bar.end_ts });
            }

            // 2. Update last prices
            self.last_prices
                .insert(bar.symbol.clone(), bar.close_micros);

            // Lazy-init risk state on first bar
            if self.risk_state.is_none() {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.risk_state = Some(RiskState::new(bar.day_id, equity, bar.reject_window_id));
            }

            // 3. Build strategy context and feed bar
            self.bar_count += 1;

            let stub = BarStub::new(bar.end_ts, bar.is_complete, bar.close_micros, bar.volume);
            self.recent_bars.push(stub);
            // Trim to max window length
            let max_len = self.config.bar_history_len;
            if self.recent_bars.len() > max_len {
                let start = self.recent_bars.len() - max_len;
                self.recent_bars = self.recent_bars.split_off(start);
            }

            let recent = RecentBarsWindow::new(self.config.bar_history_len, self.recent_bars.clone());
            let ctx = StrategyContext::new(self.config.timeframe_secs, self.bar_count, recent);

            let bar_result = self.host.on_bar(&ctx).map_err(BacktestError::StrategyHost)?;

            // 4. Check if intents should execute (shadow mode check)
            if !bar_result.intents.should_execute() {
                // Shadow mode: record equity but don't execute
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.equity_curve.push((bar.end_ts, equity));
                continue;
            }

            // 5. Convert targets to order intents
            let position_book = self.build_position_book();
            let decision = targets_to_order_intents(&position_book, &bar_result.intents.output);

            // 6. Process each order intent through allocation-cap + risk + fill
            for intent in &decision.intents {
                if self.halted {
                    break;
                }

                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );

                // ---- PATCH 13: Allocation cap enforcement (engine isolation) ----
                // Only block *risk-increasing* intents. Risk-reducing is always allowed.
                let is_risk_reducing = self.is_intent_risk_reducing(intent);
                if !is_risk_reducing {
                    // Current gross exposure at marks (deterministic).
                    let exposure = compute_exposure_micros(&self.portfolio.positions, &self.last_prices);

                    // Worst-case fill price for *this intent* (ambiguity worst-case enforced).
                    let fill_price = self.conservative_fill_price(bar, &intent.side);

                    // Conservative bound: treat full order notional as additional gross exposure.
                    // NOTE: this is intentionally pessimistic and deterministic.
                    let proposed_notional_micros: i64 = {
                        let n = (intent.qty as i128) * (fill_price as i128);
                        if n > i64::MAX as i128 {
                            i64::MAX
                        } else if n < 0 {
                            0
                        } else {
                            n as i64
                        }
                    };

                    if enforce_allocation_cap_micros(
                        equity,
                        exposure.gross_exposure_micros,
                        proposed_notional_micros,
                        self.config.max_gross_exposure_mult_micros,
                    )
                    .is_err()
                    {
                        // Allocation cap breached => deterministic reject (no fill, no halt).
                        continue;
                    }
                }
                // ---------------------------------------------------------------

                let risk_input = RiskInput {
                    day_id: bar.day_id,
                    equity_micros: equity,
                    reject_window_id: bar.reject_window_id,
                    request: RequestKind::NewOrder,
                    is_risk_reducing,
                    pdt: PdtContext::ok(),
                    kill_switch: None,
                };

                let risk_state = self.risk_state.as_mut().unwrap();
                let risk_decision = risk_evaluate(&self.risk_config, risk_state, &risk_input);

                match risk_decision.action {
                    RiskAction::Allow => {
                        // Apply conservative fill price (worst-case slippage)
                        let fill_price = self.conservative_fill_price(bar, &intent.side);
                        let pf_side = match intent.side {
                            ExecSide::Buy => PfSide::Buy,
                            ExecSide::Sell => PfSide::Sell,
                        };
                        let fill = Fill::new(intent.symbol.clone(), pf_side, intent.qty, fill_price, 0);
                        apply_fill(&mut self.portfolio, &fill);
                        self.fills.push(fill);
                    }
                    RiskAction::Reject => {
                        // Skip this intent (rejected by risk)
                    }
                    RiskAction::Halt => {
                        self.halted = true;
                        self.halt_reason = Some(format!("{:?}", risk_decision.reason));
                    }
                    RiskAction::FlattenAndHalt => {
                        // Flatten all positions, then halt
                        self.flatten_all(bar);
                        self.halted = true;
                        self.halt_reason = Some(format!("{:?}", risk_decision.reason));
                    }
                }
            }

            // 7. Record equity curve point (post-execution)
            let equity = compute_equity_micros(
                self.portfolio.cash_micros,
                &self.portfolio.positions,
                &self.last_prices,
            );
            self.equity_curve.push((bar.end_ts, equity));
        }

        Ok(BacktestReport {
            halted: self.halted,
            halt_reason: self.halt_reason.clone(),
            equity_curve: self.equity_curve.clone(),
            fills: self.fills.clone(),
            last_prices: self.last_prices.clone(),
        })
    }

    /// Build a PositionBook from current portfolio positions.
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

    /// Conservative fill price: apply slippage for worst-case pricing.
    ///
    /// Ambiguity worst-case enforcement:
    /// - BUY fills at the HIGH price (worst case for buyer), then slippage on top
    /// - SELL fills at the LOW price (worst case for seller), then slippage on top
    fn conservative_fill_price(&self, bar: &BacktestBar, side: &ExecSide) -> i64 {
        let base = match side {
            ExecSide::Buy => bar.high_micros,
            ExecSide::Sell => bar.low_micros,
        };

        let slippage_bps = self.config.stress.slippage_bps;
        if slippage_bps == 0 {
            return base;
        }

        // slippage: BUY => price goes UP (worse for buyer)
        //           SELL => price goes DOWN (worse for seller)
        let adjustment = (base as i128 * slippage_bps as i128) / 10_000i128;
        match side {
            ExecSide::Buy => {
                let result = base as i128 + adjustment;
                result.min(i64::MAX as i128) as i64
            }
            ExecSide::Sell => {
                let result = base as i128 - adjustment;
                result.max(0) as i64
            }
        }
    }

    /// Check if an order intent reduces risk (closing / reducing existing position).
    fn is_intent_risk_reducing(&self, intent: &mqk_execution::OrderIntent) -> bool {
        let current_qty = self
            .portfolio
            .positions
            .get(&intent.symbol)
            .map(|p| p.qty_signed())
            .unwrap_or(0);

        match intent.side {
            ExecSide::Buy => current_qty < 0,  // buying reduces a short
            ExecSide::Sell => current_qty > 0, // selling reduces a long
        }
    }

    /// Flatten all positions deterministically (alphabetical order by symbol).
    fn flatten_all(&mut self, bar: &BacktestBar) {
        let symbols: Vec<String> = self.portfolio.positions.keys().cloned().collect();
        for sym in symbols {
            let qty = match self.portfolio.positions.get(&sym) {
                Some(pos) => pos.qty_signed(),
                None => continue,
            };
            if qty == 0 {
                continue;
            }

            let (side, abs_qty) = if qty > 0 {
                (PfSide::Sell, qty)
            } else {
                (PfSide::Buy, -qty)
            };

            let mark = *self.last_prices.get(&sym).unwrap_or(&bar.close_micros);
            let fill = Fill::new(sym, side, abs_qty, mark, 0);
            apply_fill(&mut self.portfolio, &fill);
            self.fills.push(fill);
        }
    }
}
