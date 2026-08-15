//! BKT-FUTURE-EXECUTION-01: causal, symbol-correct future execution.
//!
//! A strategy that observes a completed bar at timestamp T must never
//! receive an economic fill from that same bar, and every order must be
//! priced only from a strictly later bar belonging to the exact target
//! symbol. This file proves that invariant end to end, per the mission's
//! required-tests checklist (Tests 1-15 plus the adversarial cross-symbol
//! fixture).
//!
//! `mqk_strategy::BarStub` (what a `Strategy` actually observes via `ctx`)
//! carries no `symbol` field — a strategy cannot know *which* symbol's bar
//! just arrived, only tick count and OHLCV. So every strategy below decides
//! *what* to order purely as a function of tick number, and the engine's
//! pending-order queue (keyed by `(symbol, signal_ts)`) is solely
//! responsible for symbol-correct, causally-later resolution.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, CommissionModel, OrderStatus,
    StressProfile,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn bar(symbol: &str, end_ts: i64, open: i64, high: i64, low: i64, close: i64) -> BacktestBar {
    BacktestBar::new(symbol, end_ts, open, high, low, close, 1_000)
}

fn flat_bar(symbol: &str, end_ts: i64, price: i64) -> BacktestBar {
    bar(symbol, end_ts, price, price, price, price)
}

/// Emits the target list explicitly registered for the current tick
/// (1-based), or an empty list (no change) if this tick has none.
///
/// `targets_to_order_intents` treats an omitted symbol as an implicit
/// target of 0, so a schedule entry is only safe to omit for a symbol whose
/// *actual* (already-filled) position is currently zero — re-stating a
/// still-*pending* (unfilled) target would instead generate a duplicate
/// order, since delta is computed against actual portfolio position, not
/// against outstanding pending orders. Each test below is annotated with
/// why its own schedule is safe under this rule.
struct TickScript {
    schedule: Vec<(u64, Vec<TargetPosition>)>,
    tick: u64,
}

impl TickScript {
    fn new(schedule: Vec<(u64, Vec<TargetPosition>)>) -> Self {
        Self { schedule, tick: 0 }
    }
}

impl Strategy for TickScript {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("tick_script", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.tick += 1;
        let targets = self
            .schedule
            .iter()
            .find(|(t, _)| *t == self.tick)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        StrategyOutput::new(targets)
    }
}

fn wide_cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000; // 5x — permissive
    cfg
}

// ---------------------------------------------------------------------------
// Test 1 — No same-bar fill
// ---------------------------------------------------------------------------

#[test]
fn test1_no_same_bar_fill() {
    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 110_000_000),
    ];
    // Ends right at the fill tick — no restate needed.
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 5)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    let f = &report.fills[0];
    assert_eq!(f.signal_ts, 1_000);
    assert!(
        f.fill_ts > f.signal_ts,
        "fill_ts ({}) must be strictly greater than signal_ts ({})",
        f.fill_ts,
        f.signal_ts
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Next target-symbol bar (exact fill timestamp)
// ---------------------------------------------------------------------------

#[test]
fn test2_fills_from_exact_next_target_symbol_bar() {
    // Three AAPL bars; tick 1 signals the buy (fills tick 2). Tick 2
    // restates the target once the fill has landed (this tick) — required
    // so tick 3 doesn't see an omitted, now-nonzero AAPL position and
    // auto-flatten it via the implicit-target-0 rule.
    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 110_000_000),
        flat_bar("AAPL", 1_120, 120_000_000),
    ];
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("AAPL", 5)]),
            (2, vec![TargetPosition::new("AAPL", 5)]),
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1, "must fill exactly once, not retry");
    assert_eq!(
        report.fills[0].fill_ts, 1_060,
        "must fill from the first later bar (1_060), not a later one (1_120)"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Correct-symbol pricing
// ---------------------------------------------------------------------------

#[test]
fn test3_correct_symbol_pricing_never_uses_other_symbol_price() {
    // AAPL@1000 high=100, AMD@1000 high=250 (same timestamp, deliberately
    // far apart). AAPL@1060 high=999 (decoy — must never price the AMD
    // order). AMD@1060 high=260 — the only bar allowed to price it.
    let bars = [
        bar("AAPL", 1_000, 100_000_000, 100_000_000, 100_000_000, 100_000_000),
        bar("AMD", 1_000, 250_000_000, 250_000_000, 250_000_000, 250_000_000),
        bar("AAPL", 1_060, 999_000_000, 999_000_000, 999_000_000, 999_000_000),
        bar("AMD", 1_060, 260_000_000, 260_000_000, 260_000_000, 260_000_000),
    ];
    // BUY AMD signalled at tick 2 (the AMD@1000 bar); AMD's only order fills
    // on the last bar of the run, so no restate is ever needed.
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            2,
            vec![TargetPosition::new("AMD", 5)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    let f = &report.fills[0];
    assert_eq!(f.symbol, "AMD");
    assert_eq!(f.signal_ts, 1_000);
    assert_eq!(f.fill_ts, 1_060, "must price from AMD's own later bar");
    assert_eq!(
        f.price_micros, 260_000_000,
        "must use AMD@1060's price, not any AAPL bar's"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — Missing target symbol delays fill
// ---------------------------------------------------------------------------

#[test]
fn test4_missing_symbol_delays_fill_no_substitution() {
    // T: AMD signal. T+60: only AAPL/SPY (no AMD). T+120: AMD reappears.
    let bars = [
        bar("AMD", 1_000, 200_000_000, 200_000_000, 200_000_000, 200_000_000),
        bar("AAPL", 1_060, 300_000_000, 300_000_000, 300_000_000, 300_000_000),
        bar("SPY", 1_060, 400_000_000, 400_000_000, 400_000_000, 400_000_000),
        bar("AMD", 1_120, 210_000_000, 210_000_000, 210_000_000, 210_000_000),
    ];
    // AMD's only order fills on the last bar of the run — no restate needed.
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AMD", 5)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    assert_eq!(
        report.fills[0].fill_ts, 1_120,
        "must wait for AMD's own bar (1_120), never substitute AAPL@1_060 or SPY@1_060"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — End-of-data unfilled
// ---------------------------------------------------------------------------

#[test]
fn test5_end_of_data_leaves_order_unfilled_not_fabricated() {
    let bars = [flat_bar("AAPL", 1_000, 100_000_000)];
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 5)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 0, "no future bar exists — no fill may be fabricated");
    assert_eq!(report.orders.len(), 1, "the attempt must remain visible in evidence");
    let o = &report.orders[0];
    assert_eq!(o.status, OrderStatus::UnfilledEndOfData);
    assert_eq!(o.symbol, "AAPL");
    assert_eq!(o.signal_ts, 1_000);
    assert_eq!(o.qty, 5);
}

// ---------------------------------------------------------------------------
// Test 6 — SELL chronology
// ---------------------------------------------------------------------------

#[test]
fn test6_sell_obeys_future_execution_chronology() {
    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 110_000_000),
        flat_bar("AAPL", 1_120, 90_000_000),
    ];
    // tick1: buy (fills tick2, before tick2's own target is evaluated).
    // tick2: sell signal (position is already 10 by the time this runs).
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("AAPL", 10)]),
            (2, vec![TargetPosition::new("AAPL", 0)]),
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 2);
    let sell = &report.fills[1];
    assert_eq!(sell.side, mqk_portfolio::Side::Sell);
    assert_eq!(sell.signal_ts, 1_060);
    assert!(
        sell.fill_ts > sell.signal_ts,
        "SELL fill_ts ({}) must be strictly greater than signal_ts ({})",
        sell.fill_ts,
        sell.signal_ts
    );
    assert_eq!(sell.fill_ts, 1_120);
}

// ---------------------------------------------------------------------------
// Test 7 — Slippage remains adverse on the future fill bar
// ---------------------------------------------------------------------------

#[test]
fn test7_slippage_remains_adverse_on_the_future_bar() {
    let mut cfg = wide_cfg();
    cfg.stress = StressProfile {
        slippage_bps: 50, // 0.5% flat floor
        volatility_mult_bps: 0,
    };

    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        bar(
            "AAPL",
            1_060,
            100_000_000,
            110_000_000, // high — BUY base
            90_000_000,  // low  — SELL base
            100_000_000,
        ),
        bar(
            "AAPL",
            1_120,
            100_000_000,
            108_000_000,
            88_000_000, // low — SELL base
            100_000_000,
        ),
    ];

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("AAPL", 10)]),
            (2, vec![TargetPosition::new("AAPL", 0)]),
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 2);
    let buy = &report.fills[0];
    let sell = &report.fills[1];

    // BUY: positive slippage must push the fill strictly above the fill
    // bar's own HIGH (never favorable).
    assert!(
        buy.price_micros > 110_000_000,
        "BUY fill {} must exceed bar 1_060's HIGH (110_000_000) under positive slippage",
        buy.price_micros
    );
    // SELL: positive slippage must push the fill strictly below the fill
    // bar's own LOW (never favorable).
    assert!(
        sell.price_micros < 88_000_000,
        "SELL fill {} must be below bar 1_120's LOW (88_000_000) under positive slippage",
        sell.price_micros
    );
}

// ---------------------------------------------------------------------------
// Test 8 — Commission charged only at fill
// ---------------------------------------------------------------------------

#[test]
fn test8_commission_charged_only_at_fill_time() {
    let mut cfg = wide_cfg();
    cfg.commission = CommissionModel {
        per_share_micros: 1_000,
        bps_of_notional: 0,
    };
    let initial = cfg.initial_cash_micros;

    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 100_000_000),
    ];
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 10)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // No commission (or any cash movement) before the fill lands.
    assert_eq!(
        report.equity_curve[0],
        (1_000, initial),
        "no commission may be charged while the order is only pending"
    );
    assert_eq!(report.fills[0].fee_micros, 10_000, "10 shares * 1_000 micros/share");
    // Fee only reduces equity once the fill actually lands (flat bar, so
    // notional cancels against the mark-to-market contribution exactly).
    assert_eq!(report.equity_curve[1], (1_060, initial - 10_000));
}

// ---------------------------------------------------------------------------
// Test 9 — Portfolio unchanged while pending
// ---------------------------------------------------------------------------

#[test]
fn test9_portfolio_unchanged_while_order_is_pending() {
    let cfg = wide_cfg();
    let initial = cfg.initial_cash_micros;

    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 100_000_000),
    ];
    let mut engine = BacktestEngine::new(cfg);
    // A large pending order (500 shares @ ~$100 = $50,000) — if it had any
    // effect on the portfolio before its fill bar, equity at bar 1 would
    // move by tens of thousands of dollars. It must not move at all.
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 500)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(
        report.equity_curve[0],
        (1_000, initial),
        "equity (and therefore position) must be completely untouched while pending"
    );
    assert_eq!(report.fills.len(), 1, "the order does fill on the next bar");
}

// ---------------------------------------------------------------------------
// Test 10 — Multiple symbols pending, independently resolved
// ---------------------------------------------------------------------------

#[test]
fn test10_multiple_symbols_pending_resolve_independently() {
    let bars = [
        bar("AAPL", 1_000, 105_000_000, 105_000_000, 105_000_000, 105_000_000), // tick1: AAPL signal
        bar("AMD", 1_000, 205_000_000, 205_000_000, 205_000_000, 205_000_000),  // tick2: AMD signal
        bar("AAPL", 1_060, 115_000_000, 115_000_000, 115_000_000, 115_000_000), // tick3: AAPL fills
        bar("AMD", 1_200, 225_000_000, 225_000_000, 225_000_000, 225_000_000),  // tick4: AMD fills
    ];
    // tick3 restates AAPL (just filled this tick) but omits AMD, which is
    // safe: AMD is still unfilled (position 0) at this point.
    // tick4 restates both (AMD just filled; AAPL must also stay restated or
    // it would be implicitly flattened).
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("AAPL", 10)]),
            (2, vec![TargetPosition::new("AMD", 10)]),
            (3, vec![TargetPosition::new("AAPL", 10)]),
            (
                4,
                vec![
                    TargetPosition::new("AAPL", 10),
                    TargetPosition::new("AMD", 10),
                ],
            ),
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 2, "each symbol fills exactly once");
    let aapl = report.fills.iter().find(|f| f.symbol == "AAPL").unwrap();
    let amd = report.fills.iter().find(|f| f.symbol == "AMD").unwrap();

    assert_eq!(aapl.signal_ts, 1_000);
    assert_eq!(aapl.fill_ts, 1_060, "AAPL resolves against its own next bar");
    assert_eq!(aapl.price_micros, 115_000_000);

    assert_eq!(amd.signal_ts, 1_000);
    assert_eq!(amd.fill_ts, 1_200, "AMD resolves against its own (later) next bar");
    assert_eq!(amd.price_micros, 225_000_000);
}

// ---------------------------------------------------------------------------
// Test 11 — Deterministic input ordering
// ---------------------------------------------------------------------------

/// (a) Two valid orderings of the same same-timestamp bar pair must produce
/// the same (symbol -> (price, signal_ts, fill_ts)) fill chronology.
///
/// The two runs use different `TickScript` schedules because a strategy
/// must explicitly restate a symbol on the exact tick it fills (see the
/// module docs) — which tick that is legitimately shifts with the physical
/// row order. That per-run bookkeeping is a test-harness artifact of tick
/// counting, not a property of the engine; the economic outcome under test
/// (which timestamp/price fills which symbol) must still be identical.
#[test]
fn test11a_same_timestamp_row_order_does_not_change_fill_chronology() {
    let signal_aapl = bar("AAPL", 940, 105_000_000, 105_000_000, 105_000_000, 105_000_000);
    let signal_amd = bar("AMD", 940, 205_000_000, 205_000_000, 205_000_000, 205_000_000);
    let fill_aapl = bar("AAPL", 1_000, 111_000_000, 111_000_000, 111_000_000, 111_000_000);
    let fill_amd = bar("AMD", 1_000, 211_000_000, 211_000_000, 211_000_000, 211_000_000);

    // Ordering A: AAPL's fill bar precedes AMD's at the shared timestamp.
    let bars_a = [
        signal_aapl.clone(),
        signal_amd.clone(),
        fill_aapl.clone(),
        fill_amd.clone(),
    ];
    // tick3 = AAPL@1000 fills here -> restate AAPL. tick4 = AMD@1000 fills -> restate both.
    let schedule_a = vec![
        (1, vec![TargetPosition::new("AAPL", 10)]),
        (2, vec![TargetPosition::new("AMD", 10)]),
        (3, vec![TargetPosition::new("AAPL", 10)]),
        (
            4,
            vec![
                TargetPosition::new("AAPL", 10),
                TargetPosition::new("AMD", 10),
            ],
        ),
    ];

    // Ordering B: the shared-timestamp pair is reversed.
    let bars_b = [signal_aapl, signal_amd, fill_amd, fill_aapl];
    // tick3 = AMD@1000 fills here -> restate AMD. tick4 = AAPL@1000 fills -> restate both.
    let schedule_b = vec![
        (1, vec![TargetPosition::new("AAPL", 10)]),
        (2, vec![TargetPosition::new("AMD", 10)]),
        (3, vec![TargetPosition::new("AMD", 10)]),
        (
            4,
            vec![
                TargetPosition::new("AAPL", 10),
                TargetPosition::new("AMD", 10),
            ],
        ),
    ];

    let mut engine_a = BacktestEngine::new(wide_cfg());
    engine_a.add_strategy(Box::new(TickScript::new(schedule_a))).unwrap();
    let report_a = engine_a.run(&bars_a).unwrap();

    let mut engine_b = BacktestEngine::new(wide_cfg());
    engine_b.add_strategy(Box::new(TickScript::new(schedule_b))).unwrap();
    let report_b = engine_b.run(&bars_b).unwrap();

    use std::collections::BTreeMap;
    fn chronology(
        report: &mqk_backtest::BacktestReport,
    ) -> BTreeMap<String, (i64, i64, i64)> {
        report
            .fills
            .iter()
            .map(|f| (f.symbol.clone(), (f.price_micros, f.signal_ts, f.fill_ts)))
            .collect()
    }

    assert_eq!(
        chronology(&report_a),
        chronology(&report_b),
        "row order of a same-timestamp pair must not change which price/timestamp fills which symbol"
    );
}

/// (b) True chronological disorder (not a same-timestamp tie) fails closed
/// rather than silently picking an execution order.
#[test]
fn test11b_genuinely_out_of_order_input_fails_closed() {
    let bars = [
        flat_bar("AAPL", 2_000, 100_000_000),
        flat_bar("AAPL", 1_000, 100_000_000), // earlier ts arriving second
    ];
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![])))
        .unwrap();
    let err = engine.run(&bars).unwrap_err();
    assert_eq!(
        err,
        BacktestError::UnsortedInput {
            end_ts: 1_000,
            prev_end_ts: 2_000,
        }
    );
}

// ---------------------------------------------------------------------------
// Test 12 — No future strategy leakage
// ---------------------------------------------------------------------------

/// Records every close price the strategy has ever observed via `ctx`,
/// keyed by observation order. Never places an order.
struct LeakProbe {
    observed_closes: std::sync::Arc<std::sync::Mutex<Vec<i64>>>,
}

impl Strategy for LeakProbe {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("leak_probe", 60)
    }
    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        if let Some(last) = ctx.recent.last() {
            self.observed_closes.lock().unwrap().push(last.close_micros);
        }
        StrategyOutput::new(vec![])
    }
}

#[test]
fn test12_future_bar_data_never_leaks_into_strategy_evaluation() {
    // Distinct, monotonically increasing closes so any leak is unambiguous.
    let bars = [
        flat_bar("AAPL", 1_000, 100_000_000),
        flat_bar("AAPL", 1_060, 200_000_000),
        flat_bar("AAPL", 1_120, 300_000_000),
    ];
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(LeakProbe {
            observed_closes: observed.clone(),
        }))
        .unwrap();
    engine.run(&bars).unwrap();

    let log = observed.lock().unwrap();
    assert_eq!(log.len(), 3);
    // At each tick, the strategy must see exactly that bar's close — never
    // a later one.
    assert_eq!(log[0], 100_000_000, "tick 1 must not see bar 2 or 3's close");
    assert_eq!(log[1], 200_000_000, "tick 2 must not see bar 3's close");
    assert_eq!(log[2], 300_000_000);
}

// ---------------------------------------------------------------------------
// Test 13 — Existing risk gate still applies
// ---------------------------------------------------------------------------

#[test]
fn test13_daily_loss_limit_halt_still_fires_at_signal_time() {
    const M: i64 = 1_000_000;
    let mut cfg = wide_cfg();
    cfg.initial_cash_micros = 10_000 * M;
    cfg.daily_loss_limit_micros = 500 * M; // $500 loss limit

    let bars = [
        flat_bar("AAPL", 1_000, 100 * M), // tick1: buy signal
        flat_bar("AAPL", 1_060, 100 * M), // tick2: buy fills here @ $100
        flat_bar("AAPL", 1_120, 40 * M),  // tick3: crash; sell signal evaluated against $9,400 equity
    ];
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("AAPL", 10)]),
            (2, vec![TargetPosition::new("AAPL", 10)]), // hold once resolved
            (3, vec![TargetPosition::new("AAPL", 0)]),  // sell intent triggers risk check
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(
        report.halted,
        "daily loss limit must still halt the engine under the new pending-order model"
    );
}

// ---------------------------------------------------------------------------
// Test 14 — Existing single-symbol strategy still runs end-to-end
// ---------------------------------------------------------------------------

#[test]
fn test14_existing_builtin_strategy_runs_end_to_end() {
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    use mqk_strategy::PluginRegistry;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let strategy = reg.instantiate("intraday_scalper").unwrap();

    let mut cfg = wide_cfg();
    cfg.timeframe_secs = 300;
    let bars: Vec<BacktestBar> = (0..20)
        .map(|i| {
            let close = 100_000_000 + i * 500_000;
            bar(
                "SPY",
                1_000 + i * 300,
                close - 200_000,
                close + 300_000,
                close - 400_000,
                close,
            )
        })
        .collect();

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(strategy).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.equity_curve.len(), bars.len());
    assert!(!report.halted, "strategy should not halt on valid data");
}

// ---------------------------------------------------------------------------
// Test 15 — Deterministic replay
// ---------------------------------------------------------------------------

#[test]
fn test15_multi_symbol_replay_is_fully_deterministic() {
    let bars = [
        bar("AAPL", 1_000, 105_000_000, 105_000_000, 105_000_000, 105_000_000),
        bar("AMD", 1_000, 205_000_000, 205_000_000, 205_000_000, 205_000_000),
        bar("AAPL", 1_060, 115_000_000, 115_000_000, 115_000_000, 115_000_000),
        bar("AMD", 1_200, 225_000_000, 225_000_000, 225_000_000, 225_000_000),
    ];
    let schedule = || {
        vec![
            (1, vec![TargetPosition::new("AAPL", 10)]),
            (2, vec![TargetPosition::new("AMD", 10)]),
            (3, vec![TargetPosition::new("AAPL", 10)]),
            (
                4,
                vec![
                    TargetPosition::new("AAPL", 10),
                    TargetPosition::new("AMD", 10),
                ],
            ),
        ]
    };

    let mut e1 = BacktestEngine::new(wide_cfg());
    e1.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let r1 = e1.run(&bars).unwrap();

    let mut e2 = BacktestEngine::new(wide_cfg());
    e2.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let r2 = e2.run(&bars).unwrap();

    assert_eq!(r1.orders.len(), r2.orders.len());
    assert_eq!(r1.fills, r2.fills, "fills (ids, prices, timestamps) must be byte-identical");
    assert_eq!(r1.equity_curve, r2.equity_curve);
    assert_eq!(r1.run_id, r2.run_id);
    for (o1, o2) in r1.orders.iter().zip(r2.orders.iter()) {
        assert_eq!(o1.order_id, o2.order_id);
        assert_eq!(o1.signal_ts, o2.signal_ts);
        assert_eq!(o1.status, o2.status);
    }
}

// ---------------------------------------------------------------------------
// High-value adversarial test — extreme decoy price must never leak across symbols
// ---------------------------------------------------------------------------

#[test]
fn high_value_extreme_decoy_price_never_prices_the_wrong_symbol() {
    let bars = [
        bar("AAPL", 1_000, 100_000_000, 100_000_000, 100_000_000, 100_000_000), // tick1
        bar("AMD", 1_000, 200_000_000, 200_000_000, 200_000_000, 200_000_000),  // tick2: AMD signal
        bar(
            "AAPL",
            1_060,
            1_000_000_000_000,
            1_000_000_000_000,
            1_000_000_000_000,
            1_000_000_000_000,
        ), // tick3: absurd decoy, no AMD bar here
        bar("AMD", 1_120, 205_000_000, 205_000_000, 205_000_000, 205_000_000), // tick4: AMD's real fill bar
    ];
    // AMD's only order fills on the last bar — no restate needed.
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            2,
            vec![TargetPosition::new("AMD", 10)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    let f = &report.fills[0];
    assert_eq!(f.symbol, "AMD");
    assert_eq!(f.fill_ts, 1_120);
    assert_eq!(
        f.price_micros, 205_000_000,
        "must fill near AMD's real price, not AAPL's absurd decoy"
    );
    assert!(
        f.price_micros < 1_000_000_000,
        "fill price {} must never approach the AAPL decoy magnitude",
        f.price_micros
    );
}
