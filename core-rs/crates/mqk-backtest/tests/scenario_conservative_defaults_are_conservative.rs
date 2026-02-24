//! PATCH F2 — Conservative defaults are conservative.
//!
//! Verifies that `BacktestConfig::conservative_defaults()` has strictly tighter
//! settings than `BacktestConfig::test_defaults()` across every safety knob.
//!
//! # Success criteria
//! - `conservative_defaults()` enables integrity (stale, gap, disagreement).
//! - Risk limits (daily loss, max drawdown) are non-zero.
//! - Slippage and volatility multiplier are non-zero (realistic fill pricing).
//! - Corporate action policy is `ForbidPeriods`, not `Allow`.
//! - PDT enforcement is on.
//! - Reject-storm threshold is tighter than test defaults.
//! - `test_defaults()` is verifiably permissive (integrity off, limits disabled).
//! - A clean backtest run with `conservative_defaults()` and a passive strategy
//!   succeeds without errors or halts.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, CorporateActionPolicy, StressProfile,
};
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Minimal no-op strategy (never trades)
// ---------------------------------------------------------------------------

struct Passive;

impl Strategy for Passive {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Passive", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Consecutive 1-minute bars — no gaps, no stale feeds, always clean.
fn clean_bars(count: usize) -> Vec<BacktestBar> {
    let base_ts = 1_700_000_000i64;
    (0..count)
        .map(|i| {
            BacktestBar::new(
                "SPY",
                base_ts + ((i as i64) + 1) * 60,
                500_000_000, // open
                510_000_000, // high
                490_000_000, // low
                505_000_000, // close
                1000,        // volume
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Conservative defaults: integrity settings
// ---------------------------------------------------------------------------

/// Integrity must be ON in conservative defaults.
#[test]
fn conservative_defaults_integrity_is_enabled() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.integrity_enabled,
        "conservative_defaults must have integrity_enabled = true"
    );
}

/// Stale threshold must be a positive, finite value (120 s matching base.yaml).
#[test]
fn conservative_defaults_stale_threshold_is_nonzero() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.integrity_stale_threshold_ticks > 0,
        "conservative_defaults must have integrity_stale_threshold_ticks > 0, got {}",
        cfg.integrity_stale_threshold_ticks
    );
}

/// Stale threshold must match the value declared in config/defaults/base.yaml
/// (runtime.stale_data_threshold_seconds: 120).
#[test]
fn conservative_defaults_stale_threshold_matches_base_yaml() {
    let cfg = BacktestConfig::conservative_defaults();
    assert_eq!(
        cfg.integrity_stale_threshold_ticks, 120,
        "stale threshold must be 120 s (mirrors base.yaml runtime.stale_data_threshold_seconds)"
    );
}

/// Gap tolerance must be 0: any missing bar halts (base.yaml fail_on_gap=true).
#[test]
fn conservative_defaults_gap_tolerance_is_zero() {
    let cfg = BacktestConfig::conservative_defaults();
    assert_eq!(
        cfg.integrity_gap_tolerance_bars, 0,
        "conservative_defaults must have integrity_gap_tolerance_bars = 0 (fail on any gap)"
    );
}

/// Feed disagreement must be enforced (base.yaml feed_disagreement_policy: HALT_NEW).
#[test]
fn conservative_defaults_feed_disagreement_enforced() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.integrity_enforce_feed_disagreement,
        "conservative_defaults must have integrity_enforce_feed_disagreement = true"
    );
}

// ---------------------------------------------------------------------------
// Conservative defaults: risk limits
// ---------------------------------------------------------------------------

/// Daily loss limit must be active (non-zero).
#[test]
fn conservative_defaults_daily_loss_limit_is_positive() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.daily_loss_limit_micros > 0,
        "conservative_defaults must have daily_loss_limit_micros > 0, got {}",
        cfg.daily_loss_limit_micros
    );
}

/// Max drawdown limit must be active (non-zero).
#[test]
fn conservative_defaults_max_drawdown_limit_is_positive() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.max_drawdown_limit_micros > 0,
        "conservative_defaults must have max_drawdown_limit_micros > 0, got {}",
        cfg.max_drawdown_limit_micros
    );
}

/// Reject-storm threshold must be tighter than the permissive test value.
#[test]
fn conservative_defaults_reject_storm_is_stricter_than_test_defaults() {
    let conservative = BacktestConfig::conservative_defaults();
    let permissive = BacktestConfig::test_defaults();
    assert!(
        conservative.reject_storm_max_rejects < permissive.reject_storm_max_rejects,
        "conservative reject_storm_max_rejects ({}) must be < test_defaults ({})",
        conservative.reject_storm_max_rejects,
        permissive.reject_storm_max_rejects
    );
}

/// PDT enforcement must be on.
#[test]
fn conservative_defaults_pdt_is_enabled() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.pdt_enabled,
        "conservative_defaults must have pdt_enabled = true"
    );
}

// ---------------------------------------------------------------------------
// Conservative defaults: slippage (realistic fill pricing)
// ---------------------------------------------------------------------------

/// Flat slippage floor must be non-zero (base.yaml execution.base_slippage_bps: 5).
#[test]
fn conservative_defaults_slippage_bps_is_positive() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.stress.slippage_bps > 0,
        "conservative_defaults must have stress.slippage_bps > 0, got {}",
        cfg.stress.slippage_bps
    );
}

/// Volatility multiplier must be non-zero (base.yaml execution.volatility_multiplier: 0.5).
#[test]
fn conservative_defaults_volatility_mult_is_positive() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.stress.volatility_mult_bps > 0,
        "conservative_defaults must have stress.volatility_mult_bps > 0, got {}",
        cfg.stress.volatility_mult_bps
    );
}

// ---------------------------------------------------------------------------
// Conservative defaults: corporate action policy
// ---------------------------------------------------------------------------

/// Corporate action policy must not be `Allow` — must be `ForbidPeriods`.
#[test]
fn conservative_defaults_corporate_action_policy_is_forbid_periods() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        matches!(
            cfg.corporate_action_policy,
            CorporateActionPolicy::ForbidPeriods(_)
        ),
        "conservative_defaults must use CorporateActionPolicy::ForbidPeriods, not Allow"
    );
}

// ---------------------------------------------------------------------------
// test_defaults is verifiably permissive (proves the two constructors differ)
// ---------------------------------------------------------------------------

/// `test_defaults` must have integrity disabled — clearly test-only.
#[test]
fn test_defaults_has_integrity_disabled() {
    let cfg = BacktestConfig::test_defaults();
    assert!(
        !cfg.integrity_enabled,
        "test_defaults must have integrity_enabled = false (test-only, not for real evaluation)"
    );
}

/// `test_defaults` must have all risk limits disabled.
#[test]
fn test_defaults_has_risk_limits_disabled() {
    let cfg = BacktestConfig::test_defaults();
    assert_eq!(
        cfg.daily_loss_limit_micros, 0,
        "test_defaults daily_loss_limit_micros must be 0 (disabled)"
    );
    assert_eq!(
        cfg.max_drawdown_limit_micros, 0,
        "test_defaults max_drawdown_limit_micros must be 0 (disabled)"
    );
}

/// `test_defaults` must have zero slippage (clean test arithmetic).
#[test]
fn test_defaults_has_zero_slippage() {
    let cfg = BacktestConfig::test_defaults();
    assert_eq!(
        cfg.stress,
        StressProfile {
            slippage_bps: 0,
            volatility_mult_bps: 0,
        },
        "test_defaults stress profile must be all-zero"
    );
}

/// `test_defaults` must use `Allow` corporate action policy.
#[test]
fn test_defaults_allows_corporate_actions() {
    let cfg = BacktestConfig::test_defaults();
    assert_eq!(
        cfg.corporate_action_policy,
        CorporateActionPolicy::Allow,
        "test_defaults must use CorporateActionPolicy::Allow"
    );
}

// ---------------------------------------------------------------------------
// Integration: clean backtest with conservative_defaults succeeds
// ---------------------------------------------------------------------------

/// A passive strategy on clean consecutive bars succeeds with conservative defaults.
///
/// This verifies that `conservative_defaults()` does not introduce spurious
/// failures on well-formed data. Integrity fires per bar but no feed is seeded
/// as a secondary heartbeat, so only the primary "backtest" feed exists and is
/// always fresh (it is updated on every bar).
#[test]
fn conservative_defaults_clean_run_succeeds() {
    let cfg = BacktestConfig::conservative_defaults();
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(Passive)).unwrap();

    let bars = clean_bars(5);
    let report = engine
        .run(&bars)
        .expect("conservative_defaults with clean bars and passive strategy must not error");

    assert!(
        !report.halted,
        "conservative_defaults clean run must not halt; halt_reason = {:?}",
        report.halt_reason
    );
    assert!(
        !report.execution_blocked,
        "conservative_defaults clean run must not block execution"
    );
    assert_eq!(
        report.fills.len(),
        0,
        "passive strategy must produce no fills"
    );
    assert_eq!(
        report.equity_curve.len(),
        5,
        "equity curve must have one entry per bar"
    );
}
