use mqk_backtest::{run_sweep, BacktestBar, BacktestConfig, SweepError, SweepGrid, SweepPoint};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BKT-BAR-VOLUME-CAPACITY-SWEEP-01: target_qty x max_participation_rate_bps
/// sweep produces a genuine BAR-VOLUME capacity curve -- rows above the
/// liquidity ceiling for a given cap show nonzero
/// rejected_liquidity_capacity_count and zero fills, rows below it fill
/// normally. This proves TARGET_QTY_CURVE and BAR_VOLUME_PARTICIPATION_CURVE
/// only -- NOT a true ADV or capital-scaling capacity curve, which remain a
/// separate, not-yet-designed capability.
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
fn sweep_dimension_default_passthrough_matches_base_config() {
    // Negative control: an empty max_participation_rate_bps dimension must
    // use base_config's own cap (mirroring volatility_mult_bps's own
    // default-passthrough), not silently reset to 0.
    let bars = two_bars(1_000);
    let base = BacktestConfig {
        liquidity: mqk_backtest::LiquidityConfig {
            max_participation_rate_bps: 100, // 1% -- deliberately tiny
        },
        ..BacktestConfig::test_defaults()
    };
    let grid = SweepGrid {
        target_qty: vec![1],
        slippage_bps: vec![0],
        volatility_mult_bps: vec![],
        max_target_qty: vec![],
        max_position_notional_usd: vec![],
        max_participation_rate_bps: vec![], // empty -> use base's 100 bps
    };
    let results = run_sweep(&bars, &base, &grid, |pt| {
        Some(Box::new(BuyOnce::new(pt.target_qty)))
    })
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].max_participation_rate_bps, 100);
}

#[test]
fn capacity_curve_sweep_shows_ceiling_where_liquidity_cap_starts_refusing() {
    // volume=100, cap=1000bps (10%) -> cap_qty=10. Sweeping target_qty
    // 5,10,15,20 must show fills below/at the ceiling and liquidity
    // rejections (zero fills) strictly above it -- this is the
    // BAR_VOLUME_PARTICIPATION_CURVE proof, not an ADV curve.
    let bars = two_bars(100);
    let base = BacktestConfig::test_defaults();
    let grid = SweepGrid {
        target_qty: vec![5, 10, 15, 20],
        slippage_bps: vec![0],
        volatility_mult_bps: vec![],
        max_target_qty: vec![],
        max_position_notional_usd: vec![],
        max_participation_rate_bps: vec![1_000],
    };
    let results = run_sweep(&bars, &base, &grid, |pt| {
        Some(Box::new(BuyOnce::new(pt.target_qty)))
    })
    .unwrap();
    assert_eq!(results.len(), 4);

    for r in &results {
        if r.target_qty <= 10 {
            assert_eq!(
                r.rejected_liquidity_capacity_count, 0,
                "target_qty={} is within the 10% cap and must fill",
                r.target_qty
            );
            assert_eq!(r.fill_count, 1);
        } else {
            assert_eq!(
                r.rejected_liquidity_capacity_count, 1,
                "target_qty={} exceeds the 10% cap and must be refused",
                r.target_qty
            );
            assert_eq!(r.fill_count, 0);
        }
    }
}

#[test]
fn negative_liquidity_cap_in_sweep_grid_rejected() {
    let bars = two_bars(1_000);
    let base = BacktestConfig::test_defaults();
    let grid = SweepGrid {
        target_qty: vec![1],
        slippage_bps: vec![0],
        volatility_mult_bps: vec![],
        max_target_qty: vec![],
        max_position_notional_usd: vec![],
        max_participation_rate_bps: vec![-1],
    };
    let err = run_sweep(&bars, &base, &grid, |pt| {
        Some(Box::new(BuyOnce::new(pt.target_qty)))
    })
    .unwrap_err();
    assert_eq!(err, SweepError::InvalidLiquidityConfig { value: -1 });
}

#[test]
fn liquidity_cap_above_10000_bps_in_sweep_grid_rejected() {
    // Mission-required range check: >100% participation is not a valid
    // configuration in a sweep grid either, and must fail closed before any
    // combination runs -- matching the engine-level 0..=10_000 bound.
    let bars = two_bars(1_000);
    let base = BacktestConfig::test_defaults();
    let grid = SweepGrid {
        target_qty: vec![1],
        slippage_bps: vec![0],
        volatility_mult_bps: vec![],
        max_target_qty: vec![],
        max_position_notional_usd: vec![],
        max_participation_rate_bps: vec![10_001],
    };
    let err = run_sweep(&bars, &base, &grid, |pt| {
        Some(Box::new(BuyOnce::new(pt.target_qty)))
    })
    .unwrap_err();
    assert_eq!(err, SweepError::InvalidLiquidityConfig { value: 10_001 });
}

#[test]
fn sweep_point_carries_max_participation_rate_bps_field() {
    // Compile-time/structural proof the field exists and round-trips.
    let pt = SweepPoint {
        target_qty: 1,
        max_target_qty: None,
        max_position_notional_usd: None,
        slippage_bps: 0,
        volatility_mult_bps: 0,
        max_participation_rate_bps: 250,
    };
    assert_eq!(pt.max_participation_rate_bps, 250);
}

// RED proof: temporarily comment out `cfg.liquidity = LiquidityConfig { ... }`
// in `run_sweep` (the wiring added by BKT-BAR-VOLUME-CAPACITY-SWEEP-01) and
// re-run `capacity_curve_sweep_shows_ceiling_where_liquidity_cap_starts_refusing`
// -- it fails (every row fills, rejected_liquidity_capacity_count stays 0
// even for target_qty=15/20), proving the test is genuinely load-bearing
// against the sweep's liquidity wiring rather than passing for an unrelated
// reason. Production bytes were restored immediately after confirming the
// failure; this crate's working tree carries no trace of the temporary edit.
