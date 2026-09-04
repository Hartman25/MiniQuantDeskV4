use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError, LiquidityConfig};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BKT-BAR-VOLUME-PARTICIPATION-CAP-01: resolving-bar volume participation
/// cap for fill admission. This is NOT ADV / average daily volume -- the
/// denominator is exactly the resolving bar's own `volume` field.
///
/// BigBuyOnce: attempts to buy a fixed quantity at bar 1, then holds.
struct BigBuyOnce {
    bar_idx: u64,
    qty: i64,
}

impl BigBuyOnce {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BigBuyOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BigBuyOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", self.qty)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

/// Two identical flat bars with a fixed `volume`, priced so a 100-share
/// order ($10,000 notional against $100k default equity) never trips the
/// unrelated allocation-cap check -- only the bar-volume participation cap
/// is under test. Bar 1 signals; bar 2 is where BKT-FUTURE-EXECUTION-01
/// resolves the fill (and where the participation cap, keyed on the
/// resolving bar's own volume, is checked).
fn two_bars(volume: i64) -> Vec<BacktestBar> {
    vec![
        BacktestBar::new(
            "SPY", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, volume,
        ),
        BacktestBar::new(
            "SPY", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, volume,
        ),
    ]
}

#[test]
fn cap_disabled_by_default_large_order_still_fills() {
    // Negative control: with the cap at its default (disabled), an order far
    // larger than the bar's own volume must still fill -- proving the cap
    // is genuinely opt-in and does not silently constrain existing callers.
    let bars = two_bars(10); // tiny volume
    let cfg = BacktestConfig::test_defaults();
    assert_eq!(cfg.liquidity, LiquidityConfig::DISABLED);

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(100))).unwrap();
    let report = engine.run(&bars).unwrap();

    // Bar 2's empty strategy output is an implicit flatten-to-zero target,
    // which (since the bar-1 order actually filled) produces its own
    // second, unrelated SELL order -- so only the bar-1 BUY order itself is
    // asserted on here, not the total order count.
    assert_eq!(report.orders[0].status, mqk_backtest::OrderStatus::Filled);
    assert_eq!(report.fills.len(), 1);
}

#[test]
fn cap_enabled_rejects_order_exceeding_bar_volume() {
    // RED/GREEN: same 100-share order, same tiny 10-share-volume bar, but
    // now with a 10% participation cap active (cap_qty = 10 * 0.10 = 1).
    // The order must be rejected outright -- no partial fill -- with the
    // dedicated liquidity-capacity status, not the generic Rejected.
    let bars = two_bars(10);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000, // 10%
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(100))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 0);
    assert_eq!(report.orders.len(), 1);
    assert_eq!(
        report.orders[0].status,
        mqk_backtest::OrderStatus::RejectedLiquidityCapacity
    );
}

#[test]
fn cap_enabled_admits_order_within_bar_volume() {
    // Same cap and order size, but a much larger bar volume raises cap_qty
    // (10_000 * 0.10 = 1_000) comfortably above the 100-share order, proving
    // the cap is a genuine threshold rather than an unconditional rejection
    // once enabled.
    let bars = two_bars(10_000);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000, // 10%
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(100))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.orders[0].status, mqk_backtest::OrderStatus::Filled);
}

#[test]
fn qty_exactly_at_cap_is_admitted() {
    // volume=1_000, rate=1_000bps (10%) -> cap_qty exactly 100. A 100-share
    // order must fill (boundary is inclusive, not exclusive).
    let bars = two_bars(1_000);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(100))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.orders[0].status, mqk_backtest::OrderStatus::Filled);
}

#[test]
fn qty_one_share_above_cap_is_rejected_liquidity_capacity() {
    // Same setup as the exact-boundary test but qty=101 -- one share past
    // cap_qty=100 -- must be refused with the liquidity-specific status.
    let bars = two_bars(1_000);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(101))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 0);
    assert_eq!(
        report.orders[0].status,
        mqk_backtest::OrderStatus::RejectedLiquidityCapacity
    );
}

#[test]
fn cap_enabled_zero_bar_volume_fails_closed() {
    // Unknown/non-positive bar volume must be treated as zero capacity while
    // the cap is active, not silently assumed unlimited.
    let bars = two_bars(0);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(1))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 0);
    assert_eq!(
        report.orders[0].status,
        mqk_backtest::OrderStatus::RejectedLiquidityCapacity
    );
}

#[test]
fn cap_enabled_negative_bar_volume_fails_closed() {
    // Adversarial fixture: a negative bar.volume (never produced by real
    // market-data providers, but not structurally impossible for
    // BacktestBar::new to construct) must also be treated as zero capacity,
    // not as "large negative therefore huge cap" via a sign error.
    let bars = two_bars(-5);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 1_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(1))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 0);
    assert_eq!(
        report.orders[0].status,
        mqk_backtest::OrderStatus::RejectedLiquidityCapacity
    );
}

#[test]
fn rate_at_10000_bps_admits_up_to_full_bar_volume() {
    // rate=10_000 (100%) -- an order exactly equal to the bar's own volume
    // must be admitted; this is the "up to 100% of positive bar volume"
    // boundary the mission requires distinct coverage for.
    let bars = two_bars(500);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 10_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(500))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.orders[0].status, mqk_backtest::OrderStatus::Filled);
}

#[test]
fn very_large_bar_volume_at_full_rate_does_not_overflow() {
    // volume near i64::MAX with rate=10_000 (100%) must compute cap_qty
    // without panicking/wrapping, and must admit an order at that ceiling.
    let huge_volume = i64::MAX / 2;
    let bars = two_bars(huge_volume);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 10_000,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    // A modest qty (kept well inside the default allocation cap's own
    // notional headroom) -- the point under test is that computing cap_qty
    // itself (huge_volume * 10_000 / 10_000) never panics/wraps, not that we
    // can actually construct an order at huge_volume shares.
    engine.add_strategy(Box::new(BigBuyOnce::new(100))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.orders[0].status, mqk_backtest::OrderStatus::Filled);
}

#[test]
fn negative_participation_rate_rejected_at_run_start() {
    let bars = two_bars(1_000);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: -1,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(1))).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(err, BacktestError::InvalidLiquidityConfig { value_bps: -1 });
}

#[test]
fn participation_rate_above_10000_bps_rejected_at_run_start() {
    // Mission-required range check: >100% participation is not a valid
    // configuration and must fail closed before any bar is processed.
    let bars = two_bars(1_000);
    let cfg = BacktestConfig {
        liquidity: LiquidityConfig {
            max_participation_rate_bps: 10_001,
        },
        ..BacktestConfig::test_defaults()
    };

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new(1))).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::InvalidLiquidityConfig { value_bps: 10_001 }
    );
}

// RED proof: temporarily comment out the cap-enforcement block in
// `BacktestEngine::resolve_one_pending_order` (the `if rate_bps > 0 { ... }`
// guard added by BKT-BAR-VOLUME-PARTICIPATION-CAP-01) and re-run
// `cap_enabled_rejects_order_exceeding_bar_volume` -- it fails (the order
// fills instead of being refused), proving the test is actually load-bearing
// against the cap wiring rather than passing for an unrelated reason.
// Production bytes were restored immediately after confirming the failure;
// this crate's working tree carries no trace of the temporary edit.
