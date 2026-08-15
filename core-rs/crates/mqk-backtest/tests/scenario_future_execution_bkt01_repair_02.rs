//! BKT-FUTURE-EXECUTION-01-REPAIR-02: closes two deterministic safety
//! defects found in independent review of the pushed
//! BKT-FUTURE-EXECUTION-01-REPAIR-01 code.
//!
//! Blocker 1 -- duplicate `(symbol, end_ts)` bars: economic execution must
//! fail closed on a second bar for the same `(symbol, end_ts)` pair,
//! matching the already-accepted `market_frame` contract, instead of
//! silently keeping whichever row happened to arrive first
//! (`BTreeMap::entry(..).or_insert(..)`).
//!
//! Blocker 2 -- same-timestamp execution batch safety: a bar must not be
//! usable to price a pending fill until the *entire* same-timestamp batch
//! (every symbol sharing that `end_ts`) has passed corporate-action and
//! integrity validation -- not just the physical row the engine's per-bar
//! loop happens to be visiting. Otherwise a sibling bar that would itself
//! be corporate-action-excluded or integrity-disarmed/halted/rejected could
//! price a fill (or even be used at all) before the engine discovers that
//! violation at that sibling's own later physical row.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, CorporateActionPolicy,
    ForbidEntry, OrderStatus,
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

/// Emits the target list registered for the current physical tick (1-based
// across the whole run, regardless of which symbol's row triggered it), or
/// an empty list if this tick has none.
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

struct Noop;

impl Strategy for Noop {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Noop", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

fn wide_cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000; // 5x — permissive, cap never binds
    cfg
}

// ---------------------------------------------------------------------------
// Blocker 1 -- duplicate (symbol, end_ts) fails closed
// ---------------------------------------------------------------------------

/// Test A: two bars for the same symbol at the same `end_ts` fail closed.
#[test]
fn duplicate_symbol_end_ts_fails_closed() {
    let bars = vec![flat_bar("AAPL", 60, 100_000_000), flat_bar("AAPL", 60, 110_000_000)];

    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(Noop)).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::DuplicateBar {
            symbol: "AAPL".to_string(),
            end_ts: 60
        }
    );
}

/// Test B: reversed price ordering of the same duplicate pair produces the
/// identical typed error -- physical row order must not change the outcome.
#[test]
fn duplicate_symbol_end_ts_reversed_order_fails_with_same_error() {
    let bars = vec![flat_bar("AAPL", 60, 110_000_000), flat_bar("AAPL", 60, 100_000_000)];

    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(Noop)).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::DuplicateBar {
            symbol: "AAPL".to_string(),
            end_ts: 60
        }
    );
}

/// Test C: exact (byte-identical) duplicate rows also fail closed -- the
/// check is by key identity alone, not by whether the rows disagree.
#[test]
fn exact_duplicate_rows_fail_closed() {
    let one = flat_bar("AAPL", 60, 100_000_000);
    let bars = vec![one.clone(), one];

    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(Noop)).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::DuplicateBar {
            symbol: "AAPL".to_string(),
            end_ts: 60
        }
    );
}

/// Test D: duplicate data fails before an ambiguous pending fill can occur.
/// A pending AAPL BUY order exists (signal at t=800); two duplicate AAPL
/// bars at t=900 carry deliberately far-apart prices. The run must abort
/// with `DuplicateBar` -- zero fills, zero report -- in both physical
/// orderings of the duplicate pair.
#[test]
fn duplicate_bar_cannot_ambiguously_price_a_pending_fill() {
    let signal = flat_bar("AAPL", 800, 100_000_000);
    let dup_low = flat_bar("AAPL", 900, 100_000_000);
    let dup_high = flat_bar("AAPL", 900, 900_000_000);

    let schedule = || vec![(1, vec![TargetPosition::new("AAPL", 10)])];

    let bars_low_first = [signal.clone(), dup_low.clone(), dup_high.clone()];
    let bars_high_first = [signal, dup_high, dup_low];

    for bars in [bars_low_first, bars_high_first] {
        let mut engine = BacktestEngine::new(wide_cfg());
        engine.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
        let err = engine.run(&bars).unwrap_err();
        assert_eq!(
            err,
            BacktestError::DuplicateBar {
                symbol: "AAPL".to_string(),
                end_ts: 900
            },
            "duplicate must abort the run before any ambiguous fill, regardless of row order"
        );
    }
}

/// Test E: same-timestamp bars for *different* symbols are not a duplicate
/// and remain fully legal -- the run completes normally with one equity
/// entry per physical row in the batch.
#[test]
fn same_timestamp_different_symbols_is_not_a_duplicate() {
    let bars = vec![
        flat_bar("AAPL", 60, 100_000_000),
        flat_bar("AMD", 60, 200_000_000),
        flat_bar("SPY", 60, 400_000_000),
    ];

    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(Noop)).unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(!report.halted);
    assert_eq!(report.equity_curve.len(), 3);
}

// ---------------------------------------------------------------------------
// Blocker 2 -- corporate-action safety: forbidden sibling cannot price a
// pending fill, regardless of physical row order
// ---------------------------------------------------------------------------

/// Shared fixture: a pending AAPL BUY order exists from a signal at t=800
/// (well outside the forbidden window). At t=900, AAPL falls inside a
/// declared forbidden corporate-action period while SPY (a valid sibling at
/// the same timestamp) does not. Run with `spy_first` controlling which of
/// the two t=900 rows physically comes first; the required outcome is
/// identical either way: AAPL's forbidden bar can never be used to price
/// the pending order, and the engine halts.
fn run_corporate_action_adversarial(spy_first: bool) -> mqk_backtest::BacktestReport {
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "AAPL", 900, 1_000,
        )]),
        ..wide_cfg()
    };

    let signal = flat_bar("AAPL", 800, 100_000_000);
    let spy = flat_bar("SPY", 900, 400_000_000);
    let aapl_forbidden = flat_bar("AAPL", 900, 100_000_000);

    let bars: Vec<BacktestBar> = if spy_first {
        vec![signal, spy, aapl_forbidden]
    } else {
        vec![signal, aapl_forbidden, spy]
    };

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 10)],
        )])))
        .unwrap();
    engine.run(&bars).unwrap()
}

#[test]
fn corporate_action_forbidden_sibling_cannot_fill_pending_order_spy_row_first() {
    let report = run_corporate_action_adversarial(true);

    assert!(report.halted, "engine must halt on the forbidden AAPL bar");
    let reason = report.halt_reason.clone().expect("halt_reason must be set");
    assert!(reason.contains("AAPL"), "halt reason must name AAPL; got: {reason}");
    assert!(report.fills.is_empty(), "zero fill may use forbidden AAPL market data");

    let aapl_order = report
        .orders
        .iter()
        .find(|o| o.symbol == "AAPL" && o.signal_ts == 800)
        .expect("the pending AAPL order must still be recorded");
    assert_eq!(aapl_order.status, OrderStatus::CanceledOnHalt);
}

#[test]
fn corporate_action_forbidden_sibling_cannot_fill_pending_order_aapl_row_first() {
    let report = run_corporate_action_adversarial(false);

    assert!(report.halted, "engine must halt on the forbidden AAPL bar");
    let reason = report.halt_reason.clone().expect("halt_reason must be set");
    assert!(reason.contains("AAPL"), "halt reason must name AAPL; got: {reason}");
    assert!(report.fills.is_empty(), "zero fill may use forbidden AAPL market data");

    let aapl_order = report
        .orders
        .iter()
        .find(|o| o.symbol == "AAPL" && o.signal_ts == 800)
        .expect("the pending AAPL order must still be recorded");
    assert_eq!(aapl_order.status, OrderStatus::CanceledOnHalt);
}

/// Physical row ordering must not decide the safety outcome: both
/// permutations above must be economically and safety-equivalent.
#[test]
fn corporate_action_adversarial_outcome_is_row_order_independent() {
    let spy_first = run_corporate_action_adversarial(true);
    let aapl_first = run_corporate_action_adversarial(false);

    assert_eq!(spy_first.halted, aapl_first.halted);
    assert_eq!(spy_first.halt_reason, aapl_first.halt_reason);
    assert_eq!(spy_first.fills, aapl_first.fills);
    assert_eq!(spy_first.equity_curve, aapl_first.equity_curve);

    let status = |r: &mqk_backtest::BacktestReport| {
        r.orders
            .iter()
            .find(|o| o.symbol == "AAPL" && o.signal_ts == 800)
            .map(|o| o.status.clone())
    };
    assert_eq!(status(&spy_first), status(&aapl_first));
}

// ---------------------------------------------------------------------------
// Blocker 2 -- integrity safety: an integrity-invalid sibling cannot price
// a pending fill before the integrity gate is known
// ---------------------------------------------------------------------------

/// Shared fixture: gap-detection integrity halt, specific to AAPL, inside a
/// same-timestamp batch that also contains a perfectly valid SPY bar.
///
/// - t=880: AAPL bar seeds `last_complete_end_ts[AAPL]` (tick 1, no signal).
/// - t=940: AAPL bar is the signal (tick 2 -> pending AAPL BUY); no gap
///   (940 is exactly the expected next end_ts after 880).
/// - t=1060: AAPL's expected next end_ts is 1000 (940 + 60); arriving at
///   1060 leaves one missing 60s slot, which exceeds
///   `integrity_gap_tolerance_bars = 0` -> `IntegrityAction::Halt` when
///   AAPL's own bar is evaluated. SPY has never been observed before, so
///   its own evaluation at t=1060 raises no gap.
///
/// `spy_first` controls which of the two t=1060 rows is physically first;
/// the required outcome (execution blocked before any resolution, AAPL
/// never fills) must be identical either way.
fn run_integrity_adversarial(spy_first: bool) -> (mqk_backtest::BacktestReport, BacktestEngine) {
    let mut cfg = wide_cfg();
    cfg.integrity_enabled = true;
    cfg.integrity_gap_tolerance_bars = 0;

    let seed = flat_bar("AAPL", 880, 100_000_000);
    let signal = flat_bar("AAPL", 940, 100_000_000);
    let spy = flat_bar("SPY", 1_060, 400_000_000);
    let aapl_gapped = flat_bar("AAPL", 1_060, 100_000_000);

    let bars: Vec<BacktestBar> = if spy_first {
        vec![seed, signal, spy, aapl_gapped]
    } else {
        vec![seed, signal, aapl_gapped, spy]
    };

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            2,
            vec![TargetPosition::new("AAPL", 10)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();
    (report, engine)
}

#[test]
fn integrity_invalid_sibling_cannot_fill_pending_order_spy_row_first() {
    let (report, engine) = run_integrity_adversarial(true);

    assert!(report.execution_blocked, "gap-detected AAPL bar must block execution");
    assert!(engine.integrity_state().halted, "integrity state must record the gap halt");
    assert!(report.fills.is_empty(), "zero fill may use the gap-invalid AAPL bar");

    let aapl_order = report
        .orders
        .iter()
        .find(|o| o.symbol == "AAPL" && o.signal_ts == 940)
        .expect("the pending AAPL order must still be recorded");
    assert_eq!(aapl_order.status, OrderStatus::UnfilledEndOfData);
}

#[test]
fn integrity_invalid_sibling_cannot_fill_pending_order_aapl_row_first() {
    let (report, engine) = run_integrity_adversarial(false);

    assert!(report.execution_blocked, "gap-detected AAPL bar must block execution");
    assert!(engine.integrity_state().halted, "integrity state must record the gap halt");
    assert!(report.fills.is_empty(), "zero fill may use the gap-invalid AAPL bar");

    let aapl_order = report
        .orders
        .iter()
        .find(|o| o.symbol == "AAPL" && o.signal_ts == 940)
        .expect("the pending AAPL order must still be recorded");
    assert_eq!(aapl_order.status, OrderStatus::UnfilledEndOfData);
}

/// Physical row ordering must not decide the integrity safety outcome.
#[test]
fn integrity_adversarial_outcome_is_row_order_independent() {
    let (spy_first, _) = run_integrity_adversarial(true);
    let (aapl_first, _) = run_integrity_adversarial(false);

    assert_eq!(spy_first.execution_blocked, aapl_first.execution_blocked);
    assert_eq!(spy_first.fills, aapl_first.fills);
    assert_eq!(spy_first.equity_curve, aapl_first.equity_curve);

    let status = |r: &mqk_backtest::BacktestReport| {
        r.orders
            .iter()
            .find(|o| o.symbol == "AAPL" && o.signal_ts == 940)
            .map(|o| o.status.clone())
    };
    assert_eq!(status(&spy_first), status(&aapl_first));
}
