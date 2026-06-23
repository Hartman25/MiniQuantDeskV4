#![forbid(unsafe_code)]

//! Deterministic backtest engine (event-sourced, replayable).
//!
//! Pipeline per bar: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
//!
//! Design constraints:
//! - No wall-clock reads, no RNG.
//! - Deterministic iteration order where possible.

use std::collections::BTreeMap;

use mqk_execution::{targets_to_order_intents, Side as ExecSide};

type PositionBook = BTreeMap<String, i64>;

use mqk_integrity::{
    evaluate_bar as integrity_evaluate_bar, Bar as IntegrityBar, BarKey, FeedId, IntegrityAction,
    IntegrityConfig, IntegrityState, Timeframe as IntegrityTimeframe,
};
use mqk_isolation::enforce_allocation_cap_micros;
use mqk_portfolio::{
    apply_fill, compute_equity_micros, compute_exposure_micros, Fill, MarkMap, PortfolioState,
    Side as PfSide,
};
use mqk_risk::{
    evaluate as risk_evaluate, PdtContext, RequestKind, RiskAction, RiskConfig, RiskInput,
    RiskState,
};
use mqk_strategy::{
    BarStub, RecentBarsWindow, ShadowMode, Strategy, StrategyContext, StrategyHost,
    StrategyHostError,
};

use crate::economics::{
    notional_micros, BacktestEconomicsLedger, BacktestEconomicsReport, BacktestInstrumentEconomics,
};
use crate::types::{
    derive_input_data_hash, derive_run_id_with_economics, BacktestBar, BacktestConfig,
    BacktestFill, BacktestOrder, BacktestOrderSide, BacktestReport, OrderStatus,
};

/// Backtest error variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BacktestError {
    /// A bar was marked incomplete (anti-lookahead).
    IncompleteBar { symbol: String, end_ts: i64 },
    /// Negative timestamp is invalid.
    NegativeTimestamp { end_ts: i64 },
    /// Strategy host error (forwarded).
    StrategyHost(StrategyHostError),
    /// PATCH F1 -- Negative slippage would make fills artificially favorable.
    ///
    /// Both `slippage_bps` and `volatility_mult_bps` must be >= 0. A negative value
    /// inverts the fill-price adjustment (BUY fills cheaper, SELL fills higher),
    /// which is a look-ahead / overfitting artifact and is unconditionally rejected.
    NegativeSlippage {
        /// The field name that carried the negative value.
        field: &'static str,
        /// The offending value in basis points.
        value_bps: i64,
    },
    /// BACKTEST-MULTIPLIER-RUN-WIRE-01 -- `contract_multiplier` must be
    /// strictly positive. Fails closed before any bar is processed or any
    /// artifact is produced. Defense-in-depth: `BacktestInstrumentEconomics`
    /// fields are public, so a caller could construct an invalid value
    /// without going through the validating `::new()` constructor.
    InvalidEconomics { multiplier: i64 },
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
            BacktestError::NegativeSlippage { field, value_bps } => write!(
                f,
                "negative slippage rejected: {} = {} bps (must be >= 0; negative values produce favorable fills and are forbidden)",
                field,
                value_bps
            ),
            BacktestError::InvalidEconomics { multiplier } => write!(
                f,
                "invalid economics rejected: contract_multiplier = {} (must be > 0)",
                multiplier
            ),
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
    /// All order intents recorded during the run (filled AND rejected).
    orders: Vec<BacktestOrder>,
    /// All fills recorded during the run (with per-fill provenance).
    fills: Vec<BacktestFill>,
    /// Equity curve: (end_ts, equity_micros).
    equity_curve: Vec<(i64, i64)>,
    /// Whether the engine has halted.
    halted: bool,
    /// Reason for halt.
    halt_reason: Option<String>,
    /// Bar counter (deterministic tick).
    bar_count: u64,
    // --- PATCH 22: integrity gate ---
    /// Integrity config (constructed from BacktestConfig).
    integrity_config: IntegrityConfig,
    /// Integrity state (tracks stale/gap/disagreement).
    integrity_state: IntegrityState,
    /// Whether integrity checks are enabled.
    integrity_enabled: bool,
    /// Whether execution is blocked due to integrity disarm/halt.
    /// Once true, no new orders are submitted for the rest of the run.
    execution_blocked: bool,
    /// Open price of the first complete bar processed (for benchmark computation).
    first_bar_open_micros: Option<i64>,
    /// Close price of the last complete bar processed (for benchmark computation).
    last_bar_close_micros: Option<i64>,
    // --- BACKTEST-MULTIPLIER-RUN-WIRE-01: backtest-only economics seam ---
    /// Economics for this run. Defaults to `BacktestInstrumentEconomics::equity()`
    /// (multiplier=1), which reproduces today's behavior exactly. Set via
    /// `with_economics` before `run()`.
    economics: BacktestInstrumentEconomics,
    /// Backtest-only shadow ledger tracking multiplier-aware cash/realized-P&L/
    /// equity in parallel with `self.portfolio`. `self.portfolio` remains the
    /// unmodified, authoritative `mqk_portfolio::PortfolioState` used for risk
    /// gating, halts, and `equity_curve`.
    economics_ledger: BacktestEconomicsLedger,
    /// Multiplier-aware equity curve, parallel to `equity_curve`. Identical to
    /// `equity_curve` when `economics.contract_multiplier == 1`.
    economics_equity_curve: Vec<(i64, i64)>,
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

        // PATCH 22 / B3: build integrity config from backtest config
        let integrity_config = IntegrityConfig {
            gap_tolerance_bars: config.integrity_gap_tolerance_bars,
            stale_threshold_ticks: config.integrity_stale_threshold_ticks,
            enforce_feed_disagreement: config.integrity_enforce_feed_disagreement,
            calendar: config.integrity_calendar,
        };
        let integrity_enabled = config.integrity_enabled;

        // BACKTEST-MULTIPLIER-RUN-WIRE-01: default to equity economics
        // (multiplier=1) so callers that never use `with_economics` get
        // exactly today's behavior, byte-for-byte.
        let economics = BacktestInstrumentEconomics::equity();
        let economics_ledger =
            BacktestEconomicsLedger::new(config.initial_cash_micros, economics.clone());

        Self {
            config,
            host,
            portfolio,
            risk_state: None,
            risk_config,
            recent_bars: Vec::new(),
            last_prices: BTreeMap::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            equity_curve: Vec::new(),
            halted: false,
            halt_reason: None,
            bar_count: 0,
            integrity_config,
            integrity_state: IntegrityState::new(),
            integrity_enabled,
            execution_blocked: false,
            first_bar_open_micros: None,
            last_bar_close_micros: None,
            economics,
            economics_ledger,
            economics_equity_curve: Vec::new(),
        }
    }

    /// BACKTEST-MULTIPLIER-RUN-WIRE-01: opt into multiplier-aware economics
    /// for this run. Must be called before `run()`. Does not touch
    /// `BacktestConfig` -- every existing `BacktestConfig` construction site
    /// (daemon, CLI, tests) is unaffected by this seam, and callers that
    /// never call this method get exactly today's equity behavior
    /// (multiplier=1, unchanged output).
    pub fn with_economics(mut self, economics: BacktestInstrumentEconomics) -> Self {
        self.economics_ledger =
            BacktestEconomicsLedger::new(self.config.initial_cash_micros, economics.clone());
        self.economics = economics;
        self
    }

    /// BACKTEST-MULTIPLIER-RUN-WIRE-01: economics configured for this run.
    pub fn economics(&self) -> &BacktestInstrumentEconomics {
        &self.economics
    }

    /// BACKTEST-MULTIPLIER-RUN-WIRE-01: multiplier-aware equity curve,
    /// parallel to `BacktestReport::equity_curve`. Identical to it when
    /// `economics().contract_multiplier == 1`.
    pub fn economics_equity_curve(&self) -> &[(i64, i64)] {
        &self.economics_equity_curve
    }

    /// BACKTEST-MULTIPLIER-RUN-WIRE-01: multiplier-aware realized P&L
    /// accumulated so far. Identical to `mqk_portfolio`'s realized P&L for
    /// the same fills when `economics().contract_multiplier == 1`.
    pub fn economics_realized_pnl_micros(&self) -> i64 {
        self.economics_ledger.realized_pnl_micros()
    }

    /// Register a strategy. Must be called before run().
    pub fn add_strategy(&mut self, s: Box<dyn Strategy>) -> Result<(), BacktestError> {
        self.host.register(s).map_err(BacktestError::StrategyHost)
    }

    /// PATCH 22: Returns true if integrity has blocked execution (disarm or halt).
    pub fn is_execution_blocked(&self) -> bool {
        self.execution_blocked
    }

    /// PATCH 22: Returns a reference to the integrity state for inspection.
    pub fn integrity_state(&self) -> &IntegrityState {
        &self.integrity_state
    }

    /// PATCH 22: Seed an additional integrity feed at a given tick.
    pub fn seed_integrity_feed(&mut self, feed_name: &str, tick: u64) {
        use mqk_integrity::tick_feed;
        let feed = FeedId::new(feed_name);
        tick_feed(
            &self.integrity_config,
            &mut self.integrity_state,
            &feed,
            tick,
        );
    }

    /// PATCH F1 -- Validate slippage knobs before executing any bars.
    fn validate_stress_profile(&self) -> Result<(), BacktestError> {
        if self.config.stress.slippage_bps < 0 {
            return Err(BacktestError::NegativeSlippage {
                field: "slippage_bps",
                value_bps: self.config.stress.slippage_bps,
            });
        }
        if self.config.stress.volatility_mult_bps < 0 {
            return Err(BacktestError::NegativeSlippage {
                field: "volatility_mult_bps",
                value_bps: self.config.stress.volatility_mult_bps,
            });
        }
        Ok(())
    }

    /// BACKTEST-MULTIPLIER-RUN-WIRE-01 -- Validate economics before
    /// executing any bars. Defense-in-depth alongside
    /// `BacktestInstrumentEconomics::new`'s own validation, since the
    /// struct's fields are public and `with_economics` accepts any value.
    fn validate_economics(&self) -> Result<(), BacktestError> {
        if self.economics.contract_multiplier <= 0 {
            return Err(BacktestError::InvalidEconomics {
                multiplier: self.economics.contract_multiplier,
            });
        }
        Ok(())
    }

    /// Run the backtest on a sequence of bars.
    pub fn run(&mut self, bars: &[BacktestBar]) -> Result<BacktestReport, BacktestError> {
        self.validate_stress_profile()?;
        self.validate_economics()?;

        // BKT-PROV-01: hash the full bar sequence before processing any bars so the
        // run identity encodes the actual input data, not just strategy + config.
        let input_data_hash = derive_input_data_hash(bars);

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

            // Corporate action exclusion gate.
            if self
                .config
                .corporate_action_policy
                .is_excluded(&bar.symbol, bar.end_ts)
            {
                self.halted = true;
                self.halt_reason = Some(format!(
                    "Corporate action exclusion: symbol '{}' at ts={} is in a forbidden period",
                    bar.symbol, bar.end_ts
                ));
                break;
            }

            // Benchmark price tracking: record first open and update last close each bar.
            if self.first_bar_open_micros.is_none() {
                self.first_bar_open_micros = Some(bar.open_micros);
            }
            self.last_bar_close_micros = Some(bar.close_micros);

            // PATCH 22: Integrity gate.
            if self.integrity_enabled {
                let feed = FeedId::new("backtest");
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
                    IntegrityAction::Disarm | IntegrityAction::Halt => {
                        self.execution_blocked = true;
                    }
                    IntegrityAction::Allow => {}
                    IntegrityAction::Reject => {
                        self.execution_blocked = true;
                    }
                }
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

            let stub = BarStub::with_ohlcv(
                bar.end_ts,
                bar.is_complete,
                bar.open_micros,
                bar.high_micros,
                bar.low_micros,
                bar.close_micros,
                bar.volume,
            );
            self.recent_bars.push(stub);
            let max_len = self.config.bar_history_len;
            if self.recent_bars.len() > max_len {
                let start = self.recent_bars.len() - max_len;
                self.recent_bars = self.recent_bars.split_off(start);
            }

            let recent =
                RecentBarsWindow::new(self.config.bar_history_len, self.recent_bars.clone());
            let ctx = StrategyContext::new(self.config.timeframe_secs, self.bar_count, recent);

            let bar_result = self
                .host
                .on_bar(&ctx)
                .map_err(BacktestError::StrategyHost)?;

            // 4. Shadow mode check
            if !bar_result.intents.should_execute() {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.equity_curve.push((bar.end_ts, equity));
                self.economics_equity_curve.push((
                    bar.end_ts,
                    self.economics_ledger.equity_micros(&self.last_prices),
                ));
                continue;
            }

            // PATCH 22: Integrity disarm gate.
            if self.execution_blocked {
                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );
                self.equity_curve.push((bar.end_ts, equity));
                self.economics_equity_curve.push((
                    bar.end_ts,
                    self.economics_ledger.equity_micros(&self.last_prices),
                ));
                continue;
            }

            // 5. Convert targets to order intents
            let position_book = self.build_position_book();
            let decision =
                targets_to_order_intents(&bar_result.intents.output.targets, &position_book);

            // PATCH C: handle HaltAndDisarm
            let intents = match decision {
                mqk_execution::ExecutionDecision::Noop => Vec::new(),
                mqk_execution::ExecutionDecision::PlaceOrders(intents) => intents,
                mqk_execution::ExecutionDecision::HaltAndDisarm { reason } => {
                    self.halted = true;
                    self.halt_reason = Some(reason);
                    Vec::new()
                }
            };

            // 6. Process each intent through allocation-cap + risk + fill.
            //
            // BKT-01P: enumerate intents so each fill carries a deterministic
            // order_id derived from (bar_end_ts, symbol, side, intent_seq).
            for (intent_seq, intent) in intents.iter().enumerate() {
                if self.halted {
                    break;
                }

                let equity = compute_equity_micros(
                    self.portfolio.cash_micros,
                    &self.portfolio.positions,
                    &self.last_prices,
                );

                // BKT-04P: compute order identity before any gate so the order
                // can be logged regardless of the outcome (cap reject, risk reject, fill).
                let is_buy = matches!(intent.side, ExecSide::Buy);
                let order_id =
                    BacktestFill::make_order_id(bar.end_ts, &intent.symbol, is_buy, intent_seq);
                let bkt_side = if is_buy {
                    BacktestOrderSide::Buy
                } else {
                    BacktestOrderSide::Sell
                };

                // PATCH 13: allocation cap enforcement — only for risk-increasing intents
                let is_risk_reducing = self.is_intent_risk_reducing(intent);
                if !is_risk_reducing {
                    let exposure =
                        compute_exposure_micros(&self.portfolio.positions, &self.last_prices);

                    let fill_price = self.conservative_fill_price(bar, &intent.side);

                    // BACKTEST-MULTIPLIER-RUN-WIRE-01: multiplier-aware
                    // notional. With `economics.contract_multiplier == 1`
                    // this is exactly `qty * fill_price` (the prior inline
                    // formula), unchanged for every existing equity backtest.
                    let proposed_notional_micros: i64 =
                        notional_micros(intent.qty, fill_price, &self.economics);

                    if enforce_allocation_cap_micros(
                        equity,
                        exposure.gross_exposure_micros,
                        proposed_notional_micros,
                        self.config.max_gross_exposure_mult_micros,
                    )
                    .is_err()
                    {
                        // BKT-04P: allocation cap rejection — log as Rejected order
                        self.orders.push(BacktestOrder {
                            order_id,
                            bar_end_ts: bar.end_ts,
                            symbol: intent.symbol.clone(),
                            side: bkt_side,
                            qty: intent.qty,
                            status: OrderStatus::Rejected,
                        });
                        continue;
                    }
                }

                let risk_input = RiskInput {
                    day_id: bar.day_id,
                    equity_micros: equity,
                    reject_window_id: bar.reject_window_id,
                    request: RequestKind::NewOrder,
                    is_risk_reducing,
                    pdt: PdtContext::ok(),
                    kill_switch: None,
                };

                let risk_state = self.risk_state.as_mut().expect("risk_state must exist");
                let risk_decision = risk_evaluate(&self.risk_config, risk_state, &risk_input);

                match risk_decision.action {
                    RiskAction::Allow => {
                        let fill_price = self.conservative_fill_price(bar, &intent.side);
                        let pf_side = match intent.side {
                            ExecSide::Buy => PfSide::Buy,
                            ExecSide::Sell => PfSide::Sell,
                        };
                        let fill_id = BacktestFill::make_fill_id(&order_id);
                        // BKT-03P: compute commission fee at fill time
                        let fee = self.config.commission.compute_fee(intent.qty, fill_price);
                        let inner =
                            Fill::new(intent.symbol.clone(), pf_side, intent.qty, fill_price, fee);
                        apply_fill(&mut self.portfolio, &inner);
                        // BACKTEST-MULTIPLIER-RUN-WIRE-01: parallel
                        // multiplier-aware shadow ledger update. Never reads
                        // from or mutates `self.portfolio`.
                        self.economics_ledger
                            .apply_fill(&intent.symbol, is_buy, intent.qty, fill_price, fee);
                        self.fills.push(BacktestFill {
                            fill_id,
                            order_id,
                            bar_end_ts: bar.end_ts,
                            inner,
                        });
                        // BKT-04P: log order as filled
                        self.orders.push(BacktestOrder {
                            order_id,
                            bar_end_ts: bar.end_ts,
                            symbol: intent.symbol.clone(),
                            side: bkt_side,
                            qty: intent.qty,
                            status: OrderStatus::Filled,
                        });
                    }
                    RiskAction::Reject => {
                        // BKT-04P: log rejected order (no fill)
                        self.orders.push(BacktestOrder {
                            order_id,
                            bar_end_ts: bar.end_ts,
                            symbol: intent.symbol.clone(),
                            side: bkt_side,
                            qty: intent.qty,
                            status: OrderStatus::Rejected,
                        });
                    }
                    RiskAction::Halt => {
                        // BKT-04P: log halt-triggering order
                        self.orders.push(BacktestOrder {
                            order_id,
                            bar_end_ts: bar.end_ts,
                            symbol: intent.symbol.clone(),
                            side: bkt_side,
                            qty: intent.qty,
                            status: OrderStatus::HaltTriggered,
                        });
                        self.halted = true;
                        self.halt_reason = Some(format!("{:?}", risk_decision.reason));
                    }
                    RiskAction::FlattenAndHalt => {
                        // BKT-04P: log halt-triggering order before flatten
                        self.orders.push(BacktestOrder {
                            order_id,
                            bar_end_ts: bar.end_ts,
                            symbol: intent.symbol.clone(),
                            side: bkt_side,
                            qty: intent.qty,
                            status: OrderStatus::HaltTriggered,
                        });
                        self.flatten_all(bar);
                        self.halted = true;
                        self.halt_reason = Some(format!("{:?}", risk_decision.reason));
                    }
                }
            }

            // 7. Record equity curve
            let equity = compute_equity_micros(
                self.portfolio.cash_micros,
                &self.portfolio.positions,
                &self.last_prices,
            );
            self.equity_curve.push((bar.end_ts, equity));
            self.economics_equity_curve.push((
                bar.end_ts,
                self.economics_ledger.equity_micros(&self.last_prices),
            ));
        }

        // BKT-PROV-01: strategy identity — derive from spec if registered.
        let strategy_name = self.host.spec().map(|s| s.name.clone()).unwrap_or_default();
        let config_id = self.config.config_id();
        // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: run identity folds in economics
        // so two runs with identical strategy/config/input but different
        // multiplier/margin cannot collide on run_id. Byte-identical to the
        // pre-existing v2 run_id whenever economics is default equity.
        let run_id = derive_run_id_with_economics(
            &strategy_name,
            &config_id,
            &input_data_hash,
            &self.economics,
        );

        // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: report.equity_curve stays the
        // unmodified mqk_portfolio-driven curve (byte-identical to every
        // existing equity backtest) when multiplier=1. For multiplier>1 it
        // surfaces the multiplier-aware economics curve instead, since the
        // mqk_portfolio-driven curve was never multiplier-aware.
        let equity_curve = if self.economics.contract_multiplier == 1 {
            self.equity_curve.clone()
        } else {
            self.economics_equity_curve.clone()
        };

        let economics_report =
            BacktestEconomicsReport::from_run(&self.economics, self.economics_realized_pnl_micros());

        Ok(BacktestReport {
            strategy_name,
            run_id,
            config_id,
            input_data_hash,
            halted: self.halted,
            halt_reason: self.halt_reason.clone(),
            equity_curve,
            orders: self.orders.clone(),
            fills: self.fills.clone(),
            last_prices: self.last_prices.clone(),
            execution_blocked: self.execution_blocked,
            first_bar_open_micros: self.first_bar_open_micros,
            last_bar_close_micros: self.last_bar_close_micros,
            sizing: self.config.sizing.clone(),
            economics: economics_report,
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
    /// - BUY fills at the HIGH price (worst case for buyer), then slippage on top.
    /// - SELL fills at the LOW price (worst case for seller), then slippage on top.
    ///
    /// Patch B5 — Slippage Realism v1
    ///
    /// Effective slippage is the sum of a flat floor and a volatility proxy:
    /// - bar_spread_bps = (high - low) * 10_000 / close
    /// - vol_component = bar_spread_bps * volatility_mult_bps / 10_000
    /// - effective_slippage_bps = slippage_bps + vol_component
    fn conservative_fill_price(&self, bar: &BacktestBar, side: &ExecSide) -> i64 {
        let base = match side {
            ExecSide::Buy => bar.high_micros,
            ExecSide::Sell => bar.low_micros,
        };

        let bar_spread_bps = if bar.close_micros > 0 {
            (bar.high_micros - bar.low_micros).saturating_mul(10_000) / bar.close_micros
        } else {
            0
        };
        let vol_component = bar_spread_bps * self.config.stress.volatility_mult_bps / 10_000;

        let effective_slippage_bps = self.config.stress.slippage_bps + vol_component;
        if effective_slippage_bps == 0 {
            return base;
        }

        let adjustment = (base as i128 * effective_slippage_bps as i128) / 10_000i128;
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

    /// Check if an execution intent reduces risk (closing / reducing existing position).
    fn is_intent_risk_reducing(&self, intent: &mqk_execution::ExecutionIntent) -> bool {
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

    /// Flatten all positions deterministically (BTreeMap alphabetical order by symbol).
    ///
    /// BKT-01P: each flatten fill carries a deterministic order_id derived from
    /// (bar_end_ts, symbol, symbol_seq) to distinguish it from intent-driven fills.
    /// BKT-04P: each flatten fill also emits a BacktestOrder with status Filled.
    fn flatten_all(&mut self, bar: &BacktestBar) {
        let symbols: Vec<String> = self.portfolio.positions.keys().cloned().collect();
        for (symbol_seq, sym) in symbols.into_iter().enumerate() {
            let qty = match self.portfolio.positions.get(&sym) {
                Some(pos) => pos.qty_signed(),
                None => continue,
            };
            if qty == 0 {
                continue;
            }

            let (pf_side, bkt_side, abs_qty) = if qty > 0 {
                (PfSide::Sell, BacktestOrderSide::Sell, qty)
            } else {
                (PfSide::Buy, BacktestOrderSide::Buy, -qty)
            };

            let mark = *self.last_prices.get(&sym).unwrap_or(&bar.close_micros);
            let order_id = BacktestFill::make_flatten_order_id(bar.end_ts, &sym, symbol_seq);
            let fill_id = BacktestFill::make_fill_id(&order_id);
            // BKT-03P: apply commission to flatten fills too
            let fee = self.config.commission.compute_fee(abs_qty, mark);
            let inner = Fill::new(sym.clone(), pf_side, abs_qty, mark, fee);
            apply_fill(&mut self.portfolio, &inner);
            // BACKTEST-MULTIPLIER-RUN-WIRE-01: parallel multiplier-aware
            // shadow ledger update, mirroring the intent-fill call site.
            self.economics_ledger.apply_fill(
                &sym,
                matches!(pf_side, PfSide::Buy),
                abs_qty,
                mark,
                fee,
            );
            self.fills.push(BacktestFill {
                fill_id,
                order_id,
                bar_end_ts: bar.end_ts,
                inner,
            });
            // BKT-04P: log flatten order as filled
            self.orders.push(BacktestOrder {
                order_id,
                bar_end_ts: bar.end_ts,
                symbol: sym,
                side: bkt_side,
                qty: abs_qty,
                status: OrderStatus::Filled,
            });
        }
    }
}

impl std::error::Error for BacktestError {}
