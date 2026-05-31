//! BACKTEST-CONFIG-DETERMINISM-SIZING-01: strategy sizing config determinism proof.
//!
//! Proves that:
//! BCD01. Default backtest behavior remains target_qty=1 (default_sizing).
//! BCD02. Explicit target_qty changes config_id.
//! BCD03. Explicit max_target_qty changes config_id.
//! BCD04. Explicit max_position_notional_usd changes config_id.
//! BCD05. Default config_id is stable (same result across calls).
//! BCD06. Default config_id differs from non-default sizing config_id.
//! BCD07. sizing.canonical_str() is deterministic and encodes all three fields.
//! BCD08. Sizing does not affect non-sizing config_id differentiation.
//! BCD09. StrategySizingConfig::default_sizing() matches test_defaults and conservative_defaults.

use mqk_backtest::{BacktestConfig, StrategySizingConfig};
use uuid::Uuid;

// ── BCD01: default_sizing is target_qty=1, no caps ───────────────────────────

#[test]
fn bcd01_default_sizing_is_target_qty_1_no_caps() {
    let sizing = StrategySizingConfig::default_sizing();
    assert_eq!(sizing.target_qty, 1, "BCD01: default target_qty must be 1");
    assert_eq!(
        sizing.max_target_qty, None,
        "BCD01: default max_target_qty must be None (no cap)"
    );
    assert_eq!(
        sizing.max_position_notional_usd, None,
        "BCD01: default max_position_notional_usd must be None (no cap)"
    );
}

// ── BCD02: target_qty changes config_id ──────────────────────────────────────

#[test]
fn bcd02_target_qty_changes_config_id() {
    let mut a = BacktestConfig::test_defaults();
    a.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: None,
        max_position_notional_usd: None,
    };

    let mut b = BacktestConfig::test_defaults();
    b.sizing = StrategySizingConfig {
        target_qty: 5,
        max_target_qty: None,
        max_position_notional_usd: None,
    };

    assert_ne!(
        a.config_id(),
        b.config_id(),
        "BCD02: target_qty=1 vs target_qty=5 must produce different config_id"
    );
}

// ── BCD03: max_target_qty changes config_id ───────────────────────────────────

#[test]
fn bcd03_max_target_qty_changes_config_id() {
    let mut a = BacktestConfig::test_defaults();
    a.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: None,
        max_position_notional_usd: None,
    };

    let mut b = BacktestConfig::test_defaults();
    b.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: Some(3),
        max_position_notional_usd: None,
    };

    assert_ne!(
        a.config_id(),
        b.config_id(),
        "BCD03: max_target_qty=None vs Some(3) must produce different config_id"
    );
}

// ── BCD04: max_position_notional_usd changes config_id ────────────────────────

#[test]
fn bcd04_max_position_notional_usd_changes_config_id() {
    let mut a = BacktestConfig::test_defaults();
    a.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: None,
        max_position_notional_usd: None,
    };

    let mut b = BacktestConfig::test_defaults();
    b.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: None,
        max_position_notional_usd: Some(10_000),
    };

    assert_ne!(
        a.config_id(),
        b.config_id(),
        "BCD04: max_position_notional_usd=None vs Some(10000) must produce different config_id"
    );
}

// ── BCD05: config_id stable across calls (no hidden state) ───────────────────

#[test]
fn bcd05_default_config_id_is_stable() {
    let cfg = BacktestConfig::test_defaults(); // default_sizing included
    let h1 = cfg.config_id();
    let h2 = cfg.config_id();
    assert_eq!(
        h1, h2,
        "BCD05: config_id must be identical across repeated calls"
    );
    assert_ne!(h1, Uuid::nil(), "BCD05: config_id must never be nil");
}

// ── BCD06: default sizing config_id differs from non-default ─────────────────

#[test]
fn bcd06_default_sizing_differs_from_nondefault() {
    let default_cfg = BacktestConfig::test_defaults();

    let mut nondefault_cfg = BacktestConfig::test_defaults();
    nondefault_cfg.sizing = StrategySizingConfig {
        target_qty: 10,
        max_target_qty: Some(5),
        max_position_notional_usd: Some(50_000),
    };

    assert_ne!(
        default_cfg.config_id(),
        nondefault_cfg.config_id(),
        "BCD06: default sizing vs explicit sizing must produce different config_id"
    );
}

// ── BCD07: canonical_str is deterministic and encodes all three fields ────────

#[test]
fn bcd07_canonical_str_is_deterministic_and_complete() {
    let s1 = StrategySizingConfig {
        target_qty: 7,
        max_target_qty: Some(4),
        max_position_notional_usd: Some(1500),
    };
    let s2 = StrategySizingConfig {
        target_qty: 7,
        max_target_qty: Some(4),
        max_position_notional_usd: Some(1500),
    };
    assert_eq!(
        s1.canonical_str(),
        s2.canonical_str(),
        "BCD07: same values → same canonical_str"
    );

    let canon = s1.canonical_str();
    assert!(
        canon.contains("sz_tgt=7"),
        "BCD07: canonical_str must encode target_qty"
    );
    assert!(
        canon.contains("sz_max_tgt=4"),
        "BCD07: canonical_str must encode max_target_qty"
    );
    assert!(
        canon.contains("sz_max_notional=1500"),
        "BCD07: canonical_str must encode max_position_notional_usd"
    );

    // None fields appear as "none"
    let s_none = StrategySizingConfig::default_sizing();
    let none_canon = s_none.canonical_str();
    assert!(
        none_canon.contains("sz_max_tgt=none"),
        "BCD07: absent max_target_qty must appear as 'none'"
    );
    assert!(
        none_canon.contains("sz_max_notional=none"),
        "BCD07: absent max_position_notional_usd must appear as 'none'"
    );
}

// ── BCD08: sizing changes are orthogonal to non-sizing config changes ─────────

#[test]
fn bcd08_sizing_change_orthogonal_to_slippage_change() {
    // Changing sizing and changing slippage both independently produce different IDs.
    let base = BacktestConfig::test_defaults();

    let mut slip_changed = BacktestConfig::test_defaults();
    slip_changed.stress.slippage_bps = 10;

    let mut size_changed = BacktestConfig::test_defaults();
    size_changed.sizing.target_qty = 3;

    let mut both_changed = BacktestConfig::test_defaults();
    both_changed.stress.slippage_bps = 10;
    both_changed.sizing.target_qty = 3;

    assert_ne!(
        base.config_id(),
        slip_changed.config_id(),
        "BCD08: slippage change must differ from base"
    );
    assert_ne!(
        base.config_id(),
        size_changed.config_id(),
        "BCD08: sizing change must differ from base"
    );
    assert_ne!(
        slip_changed.config_id(),
        size_changed.config_id(),
        "BCD08: sizing-only vs slippage-only must differ"
    );
    assert_ne!(
        both_changed.config_id(),
        slip_changed.config_id(),
        "BCD08: both-changed vs slip-only must differ"
    );
    assert_ne!(
        both_changed.config_id(),
        size_changed.config_id(),
        "BCD08: both-changed vs size-only must differ"
    );
}

// ── BCD09: test_defaults and conservative_defaults both carry default_sizing ──

#[test]
fn bcd09_both_constructors_carry_default_sizing() {
    let t = BacktestConfig::test_defaults();
    let c = BacktestConfig::conservative_defaults();

    let expected = StrategySizingConfig::default_sizing();

    assert_eq!(
        t.sizing.target_qty, expected.target_qty,
        "BCD09: test_defaults.sizing.target_qty must be default"
    );
    assert_eq!(
        t.sizing.max_target_qty, expected.max_target_qty,
        "BCD09: test_defaults.sizing.max_target_qty must be None"
    );
    assert_eq!(
        t.sizing.max_position_notional_usd, expected.max_position_notional_usd,
        "BCD09: test_defaults.sizing.max_position_notional_usd must be None"
    );

    assert_eq!(
        c.sizing.target_qty, expected.target_qty,
        "BCD09: conservative_defaults.sizing.target_qty must be default"
    );
    assert_eq!(
        c.sizing.max_target_qty, expected.max_target_qty,
        "BCD09: conservative_defaults.sizing.max_target_qty must be None"
    );
    assert_eq!(
        c.sizing.max_position_notional_usd, expected.max_position_notional_usd,
        "BCD09: conservative_defaults.sizing.max_position_notional_usd must be None"
    );
}
