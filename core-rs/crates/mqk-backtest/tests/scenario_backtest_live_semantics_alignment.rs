//! B5-2: Backtest/Live Semantics Alignment
//!
//! Proves that the execution model a strategy is evaluated against in
//! backtest is directly comparable to the live path, so that a strategy
//! promoted from backtest behaves as expected in production.
//!
//! Four alignment properties tested:
//!
//! 1. **Shared intent generation** — `targets_to_order_intents` is the same
//!    function used in both contexts; the delta rule (target − current) is
//!    proven deterministic and identical.
//!
//! 2. **Conservative fill pricing** — Backtest BUY fills at ≥ close;
//!    backtest SELL fills at ≤ close.  Any live fill at "close" or better
//!    will be at least as good, so backtest is a conservative bound.
//!
//! 3. **Shared risk evaluation** — The same `mqk_risk::evaluate` function
//!    (same config, same threshold) that halts the backtest engine also
//!    halts a direct call.  Identical rules govern both contexts.
//!
//! 4. **Shared fill accounting** — `apply_fill` is the shared ledger
//!    function.  Replaying the exact fills produced by the backtest engine
//!    through `apply_fill` directly yields identical portfolio state.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, StressProfile};
use mqk_execution::{targets_to_order_intents, Side, StrategyOutput, TargetPosition};
use mqk_integrity::CalendarSpec;
use mqk_portfolio::{apply_fill, compute_equity_micros, PortfolioState, MICROS_SCALE};
use mqk_risk::{
    evaluate as risk_evaluate, PdtContext, RequestKind, RiskAction, RiskConfig, RiskInput,
    RiskState,
};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use std::collections::BTreeMap;

const M: i64 = MICROS_SCALE;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn position_book<I>(pairs: I) -> BTreeMap<String, i64>
where
    I: IntoIterator<Item = (&'static str, i64)>,
{
    pairs
        .into_iter()
        .map(|(sym, qty)| (sym.to_string(), qty))
        .collect()
}

/// Flat bar: OHLC all equal (no spread), so fill price = price exactly.
fn flat_bar(symbol: &str, ts: i64, price_micros: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        ts,
        price_micros,
        price_micros,
        price_micros,
        price_micros,
        1_000,
    )
}

/// Non-flat bar with an explicit spread (HIGH > CLOSE > LOW).
fn spread_bar(symbol: &str, ts: i64, close_micros: i64, spread_micros: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        ts,
        close_micros,
        close_micros + spread_micros, // high
        close_micros - spread_micros, // low
        close_micros,
        1_000,
    )
}

/// Minimal config for alignment tests: known deterministic settings.
fn alignment_config(initial_cash_micros: i64, daily_loss_limit_micros: i64) -> BacktestConfig {
    BacktestConfig {
        timeframe_secs: 60,
        bar_history_len: 10,
        initial_cash_micros,
        shadow_mode: false,
        daily_loss_limit_micros,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects: 100,
        pdt_enabled: false,
        kill_switch_flattens: false,
        max_gross_exposure_mult_micros: 3_000_000, // 3x — permissive for test isolation
        stress: StressProfile {
            slippage_bps: 0,
            volatility_mult_bps: 0,
        },
        integrity_enabled: false,
        integrity_stale_threshold_ticks: 0,
        integrity_gap_tolerance_bars: 0,
        integrity_enforce_feed_disagreement: false,
        integrity_calendar: CalendarSpec::AlwaysOn,
        corporate_action_policy: mqk_backtest::CorporateActionPolicy::Allow,
    }
}

// ---------------------------------------------------------------------------
// Strategy stubs
// ---------------------------------------------------------------------------

struct StaticTarget {
    targets: Vec<(u64, Vec<TargetPosition>)>, // (tick, targets)
    tick: u64,
}

impl StaticTarget {
    fn new(targets: Vec<(u64, Vec<TargetPosition>)>) -> Self {
        Self { targets, tick: 0 }
    }
}

impl Strategy for StaticTarget {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("StaticTarget", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.tick += 1;
        let t = self
            .targets
            .iter()
            .find(|(tick, _)| *tick == self.tick)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        StrategyOutput::new(t)
    }
}

// ---------------------------------------------------------------------------
// Test 1: intent generation is the same shared function
// ---------------------------------------------------------------------------

/// Directly exercise `targets_to_order_intents` and verify the delta rule.
///
/// This is the exact same function invoked by the backtest engine on every
/// bar. Proving the rule here proves it applies in the backtest.
#[test]
fn intent_conversion_delta_rule_is_deterministic() {
    // flat → 10: BUY 10
    let book = position_book(std::iter::empty());
    let out = StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]);
    let dec = targets_to_order_intents(&out.targets, &book);
    assert_eq!(dec.intents().len(), 1);
    assert_eq!(dec.intents()[0].side, Side::Buy);
    assert_eq!(dec.intents()[0].qty, 10);

    // 10 → 5: SELL 5
    let book = position_book([("SPY", 10_i64)]);
    let out = StrategyOutput::new(vec![TargetPosition::new("SPY", 5)]);
    let dec = targets_to_order_intents(&out.targets, &book);
    assert_eq!(dec.intents().len(), 1);
    assert_eq!(dec.intents()[0].side, Side::Sell);
    assert_eq!(dec.intents()[0].qty, 5);

    // 10 → 0: SELL 10 (full close)
    let book = position_book([("SPY", 10_i64)]);
    let out = StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]);
    let dec = targets_to_order_intents(&out.targets, &book);
    assert_eq!(dec.intents().len(), 1);
    assert_eq!(dec.intents()[0].side, Side::Sell);
    assert_eq!(dec.intents()[0].qty, 10);

    // 10 → 10: no intent (already at target)
    let book = position_book([("SPY", 10_i64)]);
    let out = StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]);
    let dec = targets_to_order_intents(&out.targets, &book);
    assert_eq!(dec.intents().len(), 0);
}

/// Backtest engine applies the same delta rule: starting flat, targeting 10
/// produces a BUY 10 fill; next bar targeting 5 produces a SELL 5 fill.
#[test]
fn backtest_applies_same_delta_rule_as_direct_call() {
    let bars = vec![
        flat_bar("SPY", 1_700_000_060, 100 * M),
        flat_bar("SPY", 1_700_000_120, 100 * M),
        flat_bar("SPY", 1_700_000_180, 100 * M),
    ];

    let strategy = StaticTarget::new(vec![
        (1, vec![TargetPosition::new("SPY", 10)]), // tick 1: buy to 10
        (2, vec![TargetPosition::new("SPY", 5)]),  // tick 2: reduce to 5 (-5)
        (3, vec![TargetPosition::new("SPY", 5)]),  // tick 3: hold (no change)
    ]);

    let mut engine = BacktestEngine::new(alignment_config(100_000 * M, 0));
    engine.add_strategy(Box::new(strategy)).unwrap();
    let report = engine.run(&bars).unwrap();

    // Tick 1: BUY 10
    // Tick 2: SELL 5 (10→5)
    // Tick 3: no intent (already at 5)
    assert_eq!(report.fills.len(), 2, "expected BUY then SELL");
    assert_eq!(report.fills[0].side, mqk_portfolio::Side::Buy);
    assert_eq!(report.fills[0].qty, 10);
    assert_eq!(report.fills[1].side, mqk_portfolio::Side::Sell);
    assert_eq!(report.fills[1].qty, 5);
}

// ---------------------------------------------------------------------------
// Test 2: BUY fill price ≥ bar close (conservative, never favorable)
// ---------------------------------------------------------------------------

/// For a BUY signal, the backtest fill price is at HIGH — always ≥ CLOSE.
///
/// A live fill "at close" or "at market (between OHLC)" would be ≤ HIGH.
/// The backtest assumes the worst-case buyer price, so it is never more
/// optimistic than a live fill.
#[test]
fn buy_fill_price_is_at_least_close_conservative_bound() {
    // Spread bar: close=$100, high=$105, low=$95.
    let bar = spread_bar("SPY", 1_700_000_060, 100 * M, 5 * M);
    let close = bar.close_micros;
    let high = bar.high_micros;

    let strategy = StaticTarget::new(vec![(1, vec![TargetPosition::new("SPY", 10)])]);

    let mut engine = BacktestEngine::new(alignment_config(100_000 * M, 0));
    engine.add_strategy(Box::new(strategy)).unwrap();
    let report = engine.run(&[bar]).unwrap();

    assert_eq!(report.fills.len(), 1);
    let fill_price = report.fills[0].price_micros;

    // Fill must be at HIGH (no slippage in alignment_config).
    assert_eq!(
        fill_price, high,
        "BUY fill should be at HIGH with 0 slippage"
    );

    // Key alignment property: fill >= close (never favorable vs close).
    assert!(
        fill_price >= close,
        "BUY fill {} must be >= close {} (conservative bound)",
        fill_price,
        close
    );
}

// ---------------------------------------------------------------------------
// Test 3: SELL fill price ≤ bar close (conservative, never favorable)
// ---------------------------------------------------------------------------

/// For a SELL signal, the backtest fill price is at LOW — always ≤ CLOSE.
///
/// The backtest assumes the worst-case seller price, so it is never more
/// optimistic than a live fill.
#[test]
fn sell_fill_price_is_at_most_close_conservative_bound() {
    // Bar 1: buy 10 SPY (flat bar to avoid price complication).
    // Bar 2: sell 10 SPY from a spread bar; fill should be at LOW ≤ CLOSE.
    let buy_bar = flat_bar("SPY", 1_700_000_060, 100 * M);
    let sell_bar = spread_bar("SPY", 1_700_000_120, 100 * M, 5 * M);
    let close = sell_bar.close_micros;
    let low = sell_bar.low_micros;

    let strategy = StaticTarget::new(vec![
        (1, vec![TargetPosition::new("SPY", 10)]),
        (2, vec![TargetPosition::new("SPY", 0)]),
    ]);

    let mut engine = BacktestEngine::new(alignment_config(100_000 * M, 0));
    engine.add_strategy(Box::new(strategy)).unwrap();
    let report = engine.run(&[buy_bar, sell_bar]).unwrap();

    assert_eq!(report.fills.len(), 2);
    let sell_price = report.fills[1].price_micros;

    // Fill must be at LOW (no slippage).
    assert_eq!(
        sell_price, low,
        "SELL fill should be at LOW with 0 slippage"
    );

    // Key alignment property: fill <= close (never favorable vs close).
    assert!(
        sell_price <= close,
        "SELL fill {} must be <= close {} (conservative bound)",
        sell_price,
        close
    );
}

// ---------------------------------------------------------------------------
// Test 4: same risk config governs backtest halt and direct evaluate
// ---------------------------------------------------------------------------

/// The daily loss limit in BacktestConfig maps directly to RiskConfig.
///
/// Proof: the same threshold that causes `risk_evaluate` to return Halt
/// also causes the backtest engine to halt when the same loss is sustained.
/// Both paths use the same `mqk_risk::evaluate` function with the same config.
#[test]
fn risk_daily_loss_limit_governs_backtest_and_direct_evaluate_identically() {
    const INITIAL: i64 = 10_000 * M;
    const LIMIT: i64 = 500 * M; // $500 loss limit
                                // floor = INITIAL - LIMIT = $9,500.  Any equity <= $9,500 → Halt.

    // --- Part A: direct risk_evaluate ---
    let risk_cfg = RiskConfig {
        daily_loss_limit_micros: LIMIT,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 100,
        pdt_auto_enabled: false,
        missing_protective_stop_flattens: false,
    };
    let mut risk_state = RiskState::new(20250101, INITIAL, 0);
    let input_above = RiskInput {
        day_id: 20250101,
        equity_micros: 9_600 * M, // loss=$400 < $500 → Allow
        reject_window_id: 0,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };
    let dec = risk_evaluate(&risk_cfg, &mut risk_state, &input_above);
    assert_eq!(
        dec.action,
        RiskAction::Allow,
        "loss $400 < $500 limit should be allowed"
    );

    let mut risk_state2 = RiskState::new(20250101, INITIAL, 0);
    let input_below = RiskInput {
        day_id: 20250101,
        equity_micros: 9_400 * M, // loss=$600 > $500 → Halt
        reject_window_id: 0,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };
    let dec2 = risk_evaluate(&risk_cfg, &mut risk_state2, &input_below);
    assert_eq!(
        dec2.action,
        RiskAction::Halt,
        "loss $600 > $500 limit must halt"
    );

    // --- Part B: same threshold in backtest engine ---
    // initial=$10,000; buy 10 SPY@$100 (flat bar).  Next bar: price=$40 (crash).
    // equity = ($10,000 - $1,000) + 10*$40 = $9,400 — same as input_below above.
    let bar1 = flat_bar("SPY", 1_700_000_060, 100 * M);
    let bar2 = flat_bar("SPY", 1_700_000_120, 40 * M); // crash: equity drops to $9,400

    let strategy = StaticTarget::new(vec![
        (1, vec![TargetPosition::new("SPY", 10)]),
        (2, vec![TargetPosition::new("SPY", 0)]), // sell intent triggers risk check
    ]);

    let mut engine = BacktestEngine::new(alignment_config(INITIAL, LIMIT));
    engine.add_strategy(Box::new(strategy)).unwrap();
    let report = engine.run(&[bar1, bar2]).unwrap();

    assert!(
        report.halted,
        "backtest must halt when daily loss limit is breached (same threshold as direct evaluate)"
    );
}

// ---------------------------------------------------------------------------
// Test 5: backtest fills replay identically via shared apply_fill
// ---------------------------------------------------------------------------

/// `apply_fill` is the shared portfolio accounting function used by both the
/// backtest engine (internally) and the live daemon.
///
/// Proof: take the fills recorded by the backtest engine and replay them
/// against a fresh `PortfolioState` using `apply_fill` directly.  The
/// resulting equity and realized PnL must be identical to the engine's output.
#[test]
fn backtest_fills_replay_identically_via_shared_apply_fill() {
    const INITIAL: i64 = 100_000 * M;

    // Buy 10 SPY @ flat $100; sell at flat $110 (profit $100).
    let bars = vec![
        flat_bar("SPY", 1_700_000_060, 100 * M),
        flat_bar("SPY", 1_700_000_120, 110 * M),
    ];

    let strategy = StaticTarget::new(vec![
        (1, vec![TargetPosition::new("SPY", 10)]),
        (2, vec![TargetPosition::new("SPY", 0)]),
    ]);

    let mut engine = BacktestEngine::new(alignment_config(INITIAL, 0));
    engine.add_strategy(Box::new(strategy)).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 2, "expected buy then sell");

    // Replay the engine's fills through the shared apply_fill function.
    let mut pf = PortfolioState::new(INITIAL);
    for fill in &report.fills {
        apply_fill(&mut pf, fill);
    }

    // Equity from replay must match the last equity curve entry.
    let (_, engine_final_equity) = *report.equity_curve.last().unwrap();
    let replayed_equity = compute_equity_micros(pf.cash_micros, &pf.positions, &report.last_prices);
    assert_eq!(
        replayed_equity, engine_final_equity,
        "replayed equity {} != engine equity {}",
        replayed_equity, engine_final_equity
    );

    // Realized PnL must match: (110 - 100) * 10 = $100.
    assert_eq!(pf.realized_pnl_micros, 100 * M);
    assert!(pf.positions.is_empty(), "portfolio must be flat after sell");
}
