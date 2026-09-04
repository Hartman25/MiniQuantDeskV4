use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError, StressProfile};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BKT-BAR-VOLUME-IMPACT-STRESS-01: extra slippage proportional to how much
/// of the *resolving bar's own volume* an order consumes. This is BAR
/// participation impact, not ADV impact -- the denominator is exactly one
/// bar's own `volume` field.
///
/// BuyOnce: targets a fixed quantity at bar 1, then holds.
struct BuyOnce {
    bar_idx: u64,
    qty: i64,
}

impl BuyOnce {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BuyOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", self.qty)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

/// SellFromShortOnce: opens a short at bar 1 (so bar 2's resolving fill is a
/// BUY-to-cover)... instead we use a direct SELL by shorting: target a
/// negative position so the order side is Sell.
struct SellOnce {
    bar_idx: u64,
    qty: i64,
}

impl SellOnce {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for SellOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("SellOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", -self.qty)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

/// Two identical flat (zero-spread) bars with a fixed `volume`, so slippage
/// is driven purely by the flat floor and the impact component -- the
/// volatility component (which depends on high-low spread) is always zero.
fn two_flat_bars(volume: i64) -> Vec<BacktestBar> {
    vec![
        BacktestBar::new(
            "SPY", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, volume,
        ),
        BacktestBar::new(
            "SPY", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, volume,
        ),
    ]
}

fn run_buy_fill_price(qty: i64, volume: i64, stress: StressProfile) -> i64 {
    run_buy_fill_price_with_exposure_cap(qty, volume, stress, 1_000_000)
}

/// Like `run_buy_fill_price`, but with an explicit
/// `max_gross_exposure_mult_micros` -- needed for the overflow-safety test,
/// which deliberately drives the fill price toward `i64::MAX` and would
/// otherwise trip the unrelated allocation cap (an orthogonal risk gate,
/// not the arithmetic-safety invariant under test) before the impact
/// arithmetic itself could be observed.
fn run_buy_fill_price_with_exposure_cap(
    qty: i64,
    volume: i64,
    stress: StressProfile,
    max_gross_exposure_mult_micros: i64,
) -> i64 {
    let bars = two_flat_bars(volume);
    let cfg = BacktestConfig {
        stress,
        max_gross_exposure_mult_micros,
        ..BacktestConfig::test_defaults()
    };
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyOnce::new(qty))).unwrap();
    let report = engine.run(&bars).unwrap();
    assert_eq!(report.fills.len(), 1, "expected exactly one fill");
    report.fills[0].price_micros
}

fn run_sell_fill_price(qty: i64, volume: i64, stress: StressProfile) -> i64 {
    run_sell_fill_price_with_exposure_cap(qty, volume, stress, 1_000_000)
}

fn run_sell_fill_price_with_exposure_cap(
    qty: i64,
    volume: i64,
    stress: StressProfile,
    max_gross_exposure_mult_micros: i64,
) -> i64 {
    let bars = two_flat_bars(volume);
    let cfg = BacktestConfig {
        stress,
        max_gross_exposure_mult_micros,
        ..BacktestConfig::test_defaults()
    };
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(SellOnce::new(qty))).unwrap();
    let report = engine.run(&bars).unwrap();
    assert_eq!(report.fills.len(), 1, "expected exactly one fill");
    report.fills[0].price_micros
}

fn impact(participation_impact_bps: i64) -> StressProfile {
    StressProfile {
        slippage_bps: 0,
        volatility_mult_bps: 0,
        participation_impact_bps,
    }
}

#[test]
fn impact_disabled_by_default_price_unaffected() {
    // Negative control: with participation_impact_bps at its default (0),
    // a large-relative-to-volume order fills at the plain HIGH price (bar
    // has zero spread, so no vol_component either) -- proving the impact
    // component is genuinely opt-in.
    let cfg = BacktestConfig::test_defaults();
    assert_eq!(cfg.stress.participation_impact_bps, 0);

    let price = run_buy_fill_price(100, 10, impact(0));
    assert_eq!(price, 100_000_000, "flat bar, no stress: fills at HIGH exactly");
}

#[test]
fn impact_enabled_full_participation_moves_price_by_full_coefficient() {
    // qty == volume -> participation_bps = 10_000 (100%). With
    // participation_impact_bps = 500 (5%), impact_component = 500 bps
    // exactly, so a BUY must fill 5% above the flat $100 HIGH.
    let price = run_buy_fill_price(100, 100, impact(500));
    // base=100_000_000 micros; +5% = 105_000_000
    assert_eq!(price, 105_000_000);
}

#[test]
fn impact_enabled_half_participation_moves_price_by_half_coefficient() {
    // qty = volume / 2 -> participation_bps = 5_000 (50%). With
    // participation_impact_bps = 500 (5%), impact_component = 250 bps.
    let price = run_buy_fill_price(50, 100, impact(500));
    // base=100_000_000 micros; +2.5% = 102_500_000
    assert_eq!(price, 102_500_000);
}

#[test]
fn impact_enabled_participation_clamped_at_full_when_qty_exceeds_volume() {
    // qty = 3x volume -> participation_bps clamps at 10_000 (100%), not
    // 30_000 -- the impact component must not exceed one full application
    // of the coefficient no matter how far qty overshoots volume.
    let price_at_cap = run_buy_fill_price(300, 100, impact(500));
    let price_at_exact = run_buy_fill_price(100, 100, impact(500));
    assert_eq!(price_at_cap, price_at_exact);
}

#[test]
fn impact_enabled_zero_volume_treated_as_full_participation() {
    // Unknown/non-positive bar volume must price as worst-case (100%)
    // participation, not as zero impact.
    let price_zero_vol = run_buy_fill_price(1, 0, impact(500));
    let price_full_participation = run_buy_fill_price(100, 100, impact(500));
    assert_eq!(price_zero_vol, price_full_participation);
}

#[test]
fn buy_impact_is_monotonically_worsening_in_participation() {
    // Increasing qty (and therefore participation_bps) against a fixed
    // volume must never make a BUY fill cheaper.
    let p25 = run_buy_fill_price(25, 100, impact(1_000));
    let p50 = run_buy_fill_price(50, 100, impact(1_000));
    let p100 = run_buy_fill_price(100, 100, impact(1_000));
    assert!(p25 <= p50, "p25={p25} p50={p50}");
    assert!(p50 <= p100, "p50={p50} p100={p100}");
    assert!(p25 < p100, "impact must be strictly worse end-to-end");
}

#[test]
fn sell_impact_is_monotonically_worsening_in_participation() {
    // A SELL must fill at strictly LOWER prices as participation grows --
    // "worse" for a seller means a lower price, mirroring the BUY case.
    let p25 = run_sell_fill_price(25, 100, impact(1_000));
    let p50 = run_sell_fill_price(50, 100, impact(1_000));
    let p100 = run_sell_fill_price(100, 100, impact(1_000));
    assert!(p25 >= p50, "p25={p25} p50={p50}");
    assert!(p50 >= p100, "p50={p50} p100={p100}");
    assert!(p25 > p100, "impact must be strictly worse end-to-end");
}

#[test]
fn impact_component_is_additive_with_flat_and_volatility_stress() {
    // flat=100bps + impact-only(full participation, 500bps) must equal
    // flat+impact combined -- the components sum, neither overrides the
    // other, matching the documented effective_slippage_bps formula.
    let flat_only = run_buy_fill_price(
        100,
        100,
        StressProfile {
            slippage_bps: 100,
            volatility_mult_bps: 0,
            participation_impact_bps: 0,
        },
    );
    let impact_only = run_buy_fill_price(100, 100, impact(500));
    let combined = run_buy_fill_price(
        100,
        100,
        StressProfile {
            slippage_bps: 100,
            volatility_mult_bps: 0,
            participation_impact_bps: 500,
        },
    );
    let base = 100_000_000i64;
    assert_eq!(flat_only - base, 1_000_000); // 100bps of 100_000_000
    assert_eq!(impact_only - base, 5_000_000); // 500bps of 100_000_000
    assert_eq!(
        combined - base,
        (flat_only - base) + (impact_only - base),
        "combined slippage must equal the sum of the flat and impact components"
    );
}

#[test]
fn very_large_nonnegative_impact_coefficient_does_not_panic_or_wrap() {
    // An extreme (but validly nonnegative) coefficient must saturate to a
    // conservative price, never panic, wrap, or produce a favorable
    // (lower-than-base for BUY) price via integer overflow.
    // max_gross_exposure_mult_micros: i64::MAX so the (deliberately huge)
    // resulting notional never trips the unrelated allocation cap -- this
    // test is about the impact arithmetic's own overflow safety, not risk
    // policy.
    let price = run_buy_fill_price_with_exposure_cap(100, 100, impact(i64::MAX), i64::MAX);
    assert!(
        price >= 100_000_000,
        "an extreme nonnegative BUY impact coefficient must never produce a favorable price; got {price}"
    );

    let sell_price = run_sell_fill_price(100, 100, impact(i64::MAX));
    assert!(
        sell_price >= 0,
        "saturating SELL impact must clamp at zero, never go negative; got {sell_price}"
    );
    assert!(
        sell_price <= 100_000_000,
        "an extreme nonnegative SELL impact coefficient must never produce a favorable price; got {sell_price}"
    );
}

#[test]
fn negative_participation_impact_rejected_at_run_start() {
    let bars = two_flat_bars(1_000);
    let cfg = BacktestConfig {
        stress: impact(-1),
        ..BacktestConfig::test_defaults()
    };
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyOnce::new(1))).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::NegativeSlippage {
            field: "participation_impact_bps",
            value_bps: -1,
        }
    );
}

/// P2 identity negative control (real-engine path): two backtests identical
/// in strategy, bars, and economics but differing ONLY in
/// `participation_impact_bps` are economically different runs (the fill
/// price is genuinely worse) and must never collide on `config_id`/`run_id`.
/// Uses the real `BacktestEngine::run` production path -- `report.config_id`
/// and `report.run_id` are read directly off the report, not re-derived by
/// the test.
#[test]
fn participation_impact_alone_changes_config_id_and_run_id_via_real_engine() {
    let bars = two_flat_bars(100);

    let cfg_a = BacktestConfig {
        stress: impact(0),
        max_gross_exposure_mult_micros: 1_000_000,
        ..BacktestConfig::test_defaults()
    };
    let mut engine_a = BacktestEngine::new(cfg_a);
    engine_a.add_strategy(Box::new(BuyOnce::new(100))).unwrap();
    let report_a = engine_a.run(&bars).unwrap();

    let cfg_b = BacktestConfig {
        stress: impact(500),
        max_gross_exposure_mult_micros: 1_000_000,
        ..BacktestConfig::test_defaults()
    };
    let mut engine_b = BacktestEngine::new(cfg_b);
    engine_b.add_strategy(Box::new(BuyOnce::new(100))).unwrap();
    let report_b = engine_b.run(&bars).unwrap();

    assert_ne!(
        report_a.config_id, report_b.config_id,
        "participation_impact_bps alone must change config_id"
    );
    assert_ne!(
        report_a.run_id, report_b.run_id,
        "participation_impact_bps alone must change run_id"
    );

    assert_eq!(report_a.fills.len(), 1, "expected exactly one fill (a)");
    assert_eq!(report_b.fills.len(), 1, "expected exactly one fill (b)");
    assert!(
        report_b.fills[0].price_micros > report_a.fills[0].price_micros,
        "nonzero participation impact must strictly worsen (raise) the real \
         engine's BUY fill price: a={} b={}",
        report_a.fills[0].price_micros,
        report_b.fills[0].price_micros
    );
}

// RED proof (pricing): temporarily short-circuit the impact_component
// computation in `BacktestEngine::conservative_fill_price` (the block added
// by BKT-BAR-VOLUME-IMPACT-STRESS-01) to always yield 0 and re-run
// `impact_enabled_full_participation_moves_price_by_full_coefficient` -- it
// fails (price stays at the flat 100_000_000 base instead of 105_000_000),
// proving the test is genuinely load-bearing against the impact wiring
// rather than passing for an unrelated reason. Production bytes were
// restored immediately after confirming the failure; this crate's working
// tree carries no trace of the temporary edit.
//
// RED proof (identity): temporarily dropping the `impact={impact}` term from
// `BacktestConfig::config_id()`'s canonical string (reverting to the
// pre-patch format) makes
// `participation_impact_alone_changes_config_id_and_run_id_via_real_engine`
// fail on both the `config_id` and `run_id` assertions, while
// `impact_enabled_full_participation_moves_price_by_full_coefficient` (the
// pricing test) still passes unchanged -- proving the identity test
// specifically protects semantic-identity binding, not merely detecting
// some other difference in output. Production bytes were restored
// immediately after confirming the failure; this crate's working tree
// carries no trace of the temporary edit.
