/// PATCH F1 — Negative slippage / favorable fills must be rejected fast.
///
/// Success criteria:
/// - Attempting to run a backtest with `slippage_bps < 0` fails immediately
///   with `BacktestError::NegativeSlippage { field: "slippage_bps", .. }`.
/// - Attempting to run a backtest with `volatility_mult_bps < 0` fails immediately
///   with `BacktestError::NegativeSlippage { field: "volatility_mult_bps", .. }`.
/// - Zero values for both fields are accepted (no slippage = neutral, not favorable).
/// - Positive values are accepted (conservative-only stress knobs).
use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError, StressProfile};
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// --------------------------------------------------------------------------
// Minimal no-op strategy (never trades)
// --------------------------------------------------------------------------

struct Passive;

impl Strategy for Passive {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Passive", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

// --------------------------------------------------------------------------
// A single valid bar — used only for engines that should reach run()
// --------------------------------------------------------------------------

fn one_bar() -> Vec<BacktestBar> {
    vec![BacktestBar::new(
        "SPY",
        1_700_000_060,
        500_000_000,
        510_000_000,
        490_000_000,
        505_000_000,
        1000,
    )]
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

/// Negative `slippage_bps` is unconditionally rejected at run() entry.
#[test]
fn negative_slippage_bps_rejected() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: -1,
        volatility_mult_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let err = engine.run(&one_bar()).unwrap_err();
    match err {
        BacktestError::NegativeSlippage { field, value_bps } => {
            assert_eq!(field, "slippage_bps");
            assert_eq!(value_bps, -1);
        }
        other => panic!("expected NegativeSlippage, got {:?}", other),
    }
}

/// Large negative `slippage_bps` is also rejected.
#[test]
fn large_negative_slippage_bps_rejected() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: -9999,
        volatility_mult_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let err = engine.run(&one_bar()).unwrap_err();
    match err {
        BacktestError::NegativeSlippage { field, value_bps } => {
            assert_eq!(field, "slippage_bps");
            assert_eq!(value_bps, -9999);
        }
        other => panic!("expected NegativeSlippage, got {:?}", other),
    }
}

/// Negative `volatility_mult_bps` is unconditionally rejected at run() entry.
#[test]
fn negative_volatility_mult_bps_rejected() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: 0,
        volatility_mult_bps: -500,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let err = engine.run(&one_bar()).unwrap_err();
    match err {
        BacktestError::NegativeSlippage { field, value_bps } => {
            assert_eq!(field, "volatility_mult_bps");
            assert_eq!(value_bps, -500);
        }
        other => panic!("expected NegativeSlippage, got {:?}", other),
    }
}

/// When both fields are negative, `slippage_bps` is checked first (deterministic order).
#[test]
fn both_negative_slippage_bps_checked_first() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: -10,
        volatility_mult_bps: -20,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let err = engine.run(&one_bar()).unwrap_err();
    match err {
        BacktestError::NegativeSlippage { field, .. } => {
            assert_eq!(
                field, "slippage_bps",
                "slippage_bps must be validated first"
            );
        }
        other => panic!("expected NegativeSlippage, got {:?}", other),
    }
}

/// Zero values for both fields are accepted (neutral, not favorable).
#[test]
fn zero_slippage_accepted() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: 0,
        volatility_mult_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    // Must succeed without error.
    engine
        .run(&one_bar())
        .expect("zero slippage should be accepted");
}

/// Positive `slippage_bps` is accepted (conservative-only stress knob).
#[test]
fn positive_slippage_accepted() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: 200,
        volatility_mult_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    engine
        .run(&one_bar())
        .expect("positive slippage_bps should be accepted");
}

/// Positive `volatility_mult_bps` is accepted (conservative-only stress knob).
#[test]
fn positive_volatility_mult_accepted() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: 0,
        volatility_mult_bps: 5_000,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    engine
        .run(&one_bar())
        .expect("positive volatility_mult_bps should be accepted");
}

/// Error message is human-readable and contains the field name and value.
#[test]
fn negative_slippage_error_message_is_informative() {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.stress = StressProfile {
        slippage_bps: -42,
        volatility_mult_bps: 0,
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let err = engine.run(&one_bar()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("slippage_bps"),
        "error message should contain field name; got: {}",
        msg
    );
    assert!(
        msg.contains("-42"),
        "error message should contain the bad value; got: {}",
        msg
    );
}
