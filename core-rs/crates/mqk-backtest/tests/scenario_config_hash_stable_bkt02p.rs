//! BKT-02P: BacktestConfig identity hash proof.
//!
//! Proves that `BacktestConfig::config_id()` is:
//! - Stable: same config → same UUID on every call (no hidden state)
//! - Non-nil: never produces the nil UUID
//! - Sensitive: any changed field produces a different UUID
//! - Discriminating: test_defaults and conservative_defaults produce different IDs

use mqk_backtest::BacktestConfig;
use uuid::Uuid;

#[test]
fn config_id_is_stable_across_calls() {
    let cfg = BacktestConfig::test_defaults();
    let h1 = cfg.config_id();
    let h2 = cfg.config_id();
    assert_eq!(
        h1, h2,
        "config_id must be identical across calls on same config"
    );
}

#[test]
fn config_id_is_not_nil() {
    assert_ne!(
        BacktestConfig::test_defaults().config_id(),
        Uuid::nil(),
        "test_defaults config_id must not be nil"
    );
    assert_ne!(
        BacktestConfig::conservative_defaults().config_id(),
        Uuid::nil(),
        "conservative_defaults config_id must not be nil"
    );
}

#[test]
fn test_defaults_and_conservative_defaults_have_different_ids() {
    let t = BacktestConfig::test_defaults().config_id();
    let c = BacktestConfig::conservative_defaults().config_id();
    assert_ne!(
        t, c,
        "test_defaults and conservative_defaults must have distinct config IDs"
    );
}

#[test]
fn config_id_differs_on_changed_timeframe_secs() {
    let a = BacktestConfig::test_defaults();
    let mut b = BacktestConfig::test_defaults();
    b.timeframe_secs = 300;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "timeframe_secs change must produce different ID"
    );
}

#[test]
fn config_id_differs_on_changed_initial_cash() {
    let a = BacktestConfig::test_defaults();
    let mut b = BacktestConfig::test_defaults();
    b.initial_cash_micros = 50_000_000_000;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "initial_cash_micros change must produce different ID"
    );
}

#[test]
fn config_id_differs_on_changed_slippage() {
    let a = BacktestConfig::test_defaults();
    let mut b = BacktestConfig::test_defaults();
    b.stress.slippage_bps = 10;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "slippage_bps change must produce different ID"
    );
}

#[test]
fn config_id_differs_on_changed_volatility_mult() {
    let a = BacktestConfig::test_defaults();
    let mut b = BacktestConfig::test_defaults();
    b.stress.volatility_mult_bps = 5_000;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "volatility_mult_bps change must produce different ID"
    );
}

#[test]
fn config_id_differs_on_enabled_integrity() {
    let a = BacktestConfig::test_defaults(); // integrity_enabled=false
    let mut b = BacktestConfig::test_defaults();
    b.integrity_enabled = true;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "integrity_enabled change must produce different ID"
    );
}

#[test]
fn config_id_differs_on_changed_daily_loss_limit() {
    let a = BacktestConfig::test_defaults();
    let mut b = BacktestConfig::test_defaults();
    b.daily_loss_limit_micros = 1_000_000_000;
    assert_ne!(
        a.config_id(),
        b.config_id(),
        "daily_loss_limit_micros change must produce different ID"
    );
}
