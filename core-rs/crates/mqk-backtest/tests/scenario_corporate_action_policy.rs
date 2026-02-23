//! Patch B4 — Corporate action policy scenario tests.
//!
//! Validates that:
//! - `CorporateActionPolicy::Allow` never blocks bars (backward compatible).
//! - `CorporateActionPolicy::ForbidPeriods` halts immediately when a bar
//!   arrives for a symbol in a declared exclusion period.
//! - Forbidden periods are symbol-specific (other symbols proceed normally).
//! - Bars outside the forbidden period are not affected.
//! - The halt reason message identifies the exclusion.
//! - The `is_excluded` predicate itself behaves correctly (unit coverage).

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, CorporateActionPolicy, ForbidEntry,
};
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Minimal no-op strategy
// ---------------------------------------------------------------------------

struct Noop;

impl Strategy for Noop {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Noop", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bar(symbol: &str, end_ts: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        100_000_000,
        110_000_000,
        90_000_000,
        105_000_000,
        1000,
    )
}

fn run(cfg: BacktestConfig, bars: Vec<BacktestBar>) -> mqk_backtest::BacktestReport {
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Noop)).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// Scenario 1: Allow policy — no bars blocked
// ---------------------------------------------------------------------------

/// `CorporateActionPolicy::Allow` must never halt the backtest, even when
/// bar timestamps coincide with periods that *would* be forbidden under
/// `ForbidPeriods`.
#[test]
fn allow_policy_does_not_block_any_bar() {
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::Allow,
        ..BacktestConfig::test_defaults()
    };

    let bars = vec![
        bar("SPY", 1_000_000),
        bar("SPY", 1_000_060),
        bar("SPY", 1_000_120),
    ];

    let report = run(cfg, bars);
    assert!(!report.halted, "Allow policy must not halt");
    assert_eq!(report.halt_reason, None);
    assert_eq!(report.equity_curve.len(), 3, "all 3 bars processed");
}

// ---------------------------------------------------------------------------
// Scenario 2: ForbidPeriods — bar in forbidden period halts immediately
// ---------------------------------------------------------------------------

/// When a bar's (symbol, end_ts) falls within a declared exclusion window,
/// the backtest must halt immediately — without processing further bars.
#[test]
fn forbid_period_halts_before_further_bars_run() {
    let forbidden_start = 1_000_060_i64;
    let forbidden_end = 1_000_180_i64;

    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "AAPL",
            forbidden_start,
            forbidden_end,
        )]),
        ..BacktestConfig::test_defaults()
    };

    let bars = vec![
        bar("AAPL", 1_000_000), // OK: before forbidden window
        bar("AAPL", 1_000_060), // hits forbidden window → halt
        bar("AAPL", 1_000_120), // must NOT be processed
    ];

    let report = run(cfg, bars);
    assert!(report.halted, "engine must halt on forbidden bar");
    // Equity curve should have exactly 1 entry (first bar processed, halt on second)
    assert_eq!(
        report.equity_curve.len(),
        1,
        "only the first bar should produce an equity entry; got {}",
        report.equity_curve.len()
    );
}

// ---------------------------------------------------------------------------
// Scenario 3: ForbidPeriods — other symbols proceed normally
// ---------------------------------------------------------------------------

/// A forbidden period is symbol-specific. Bars for other symbols in the
/// same timestamp range must not be blocked.
#[test]
fn forbid_period_does_not_affect_other_symbols() {
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "AAPL", 1_000_000, 1_001_000,
        )]),
        ..BacktestConfig::test_defaults()
    };

    // SPY bars with timestamps inside AAPL's forbidden window → must be allowed
    let bars = vec![
        bar("SPY", 1_000_060),
        bar("SPY", 1_000_120),
        bar("SPY", 1_000_180),
    ];

    let report = run(cfg, bars);
    assert!(!report.halted, "SPY must not be blocked by AAPL exclusion");
    assert_eq!(report.equity_curve.len(), 3, "all SPY bars processed");
}

// ---------------------------------------------------------------------------
// Scenario 4: ForbidPeriods — bars outside the window proceed normally
// ---------------------------------------------------------------------------

/// Bars for the correct symbol but *outside* the forbidden period must not
/// be blocked.
#[test]
fn forbid_outside_period_does_not_block() {
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "SPY", 2_000_000, 2_001_000,
        )]),
        ..BacktestConfig::test_defaults()
    };

    // All bars are well before the forbidden window
    let bars = vec![
        bar("SPY", 1_000_000),
        bar("SPY", 1_000_060),
        bar("SPY", 1_000_120),
    ];

    let report = run(cfg, bars);
    assert!(!report.halted, "bars before forbidden period must not halt");
    assert_eq!(report.equity_curve.len(), 3);
}

// ---------------------------------------------------------------------------
// Scenario 5: Halt reason identifies corporate action exclusion
// ---------------------------------------------------------------------------

/// The halt reason string must name the affected symbol and timestamp so
/// that post-mortem analysis can identify the exclusion.
#[test]
fn halt_reason_identifies_corporate_action_exclusion() {
    let cfg = BacktestConfig {
        corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
            "MSFT", 5_000_000, 5_001_000,
        )]),
        ..BacktestConfig::test_defaults()
    };

    let bars = vec![bar("MSFT", 5_000_000)];
    let report = run(cfg, bars);

    assert!(report.halted);
    let reason = report.halt_reason.expect("halt_reason must be set");
    assert!(
        reason.contains("MSFT"),
        "halt reason must name the symbol; got: {reason}"
    );
    assert!(
        reason.contains("5000000") || reason.contains("5_000_000"),
        "halt reason must include the timestamp; got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 6: is_excluded unit assertions
// ---------------------------------------------------------------------------

/// `CorporateActionPolicy::Allow.is_excluded()` always returns false.
#[test]
fn allow_is_excluded_is_always_false() {
    let policy = CorporateActionPolicy::Allow;
    assert!(!policy.is_excluded("SPY", 0));
    assert!(!policy.is_excluded("SPY", i64::MAX));
    assert!(!policy.is_excluded("ANYTHING", 1_000_000));
}

/// `ForbidPeriods.is_excluded()` respects symbol and inclusive bounds.
#[test]
fn forbid_is_excluded_respects_bounds() {
    let policy = CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new("SPY", 100, 200)]);

    // Exactly at start_ts — excluded (inclusive)
    assert!(
        policy.is_excluded("SPY", 100),
        "start_ts must be excluded (inclusive)"
    );
    // Mid-window — excluded
    assert!(
        policy.is_excluded("SPY", 150),
        "mid-window must be excluded"
    );
    // Exactly at end_ts — excluded (inclusive)
    assert!(
        policy.is_excluded("SPY", 200),
        "end_ts must be excluded (inclusive)"
    );

    // One second before start — not excluded
    assert!(
        !policy.is_excluded("SPY", 99),
        "one before start must not be excluded"
    );
    // One second after end — not excluded
    assert!(
        !policy.is_excluded("SPY", 201),
        "one after end must not be excluded"
    );

    // Different symbol — not excluded even if timestamp matches
    assert!(
        !policy.is_excluded("AAPL", 150),
        "different symbol must not be excluded"
    );
}
