//! Scenario: Negative or zero order quantity halts — Patch L10
//!
//! # Invariants under test
//!
//! 1. `validate_order_qty(-1)`      → Halt / BadInput
//! 2. `validate_order_qty(0)`       → Halt / BadInput  (zero is meaningless)
//! 3. `validate_order_qty(1)`       → None  (positive is valid)
//! 4. `validate_order_qty(i64::MIN)` → Halt / BadInput  (extreme negative, no panic)
//! 5. `validate_order_qty(i64::MAX)` → None  (maximum positive is valid)
//! 6. An already-halted engine rejects new orders — negative qty cannot bypass
//!    the sticky halt that was set from a prior bad-input event.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_risk::*;

const M: i64 = 1_000_000;

// ---------------------------------------------------------------------------
// 1. Negative quantity → bad input
// ---------------------------------------------------------------------------

#[test]
fn negative_order_qty_is_bad_input() {
    let d = validate_order_qty(-1).expect("negative qty must be detected as bad input");
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::BadInput);
}

// ---------------------------------------------------------------------------
// 2. Zero quantity → bad input (meaningless order)
// ---------------------------------------------------------------------------

#[test]
fn zero_order_qty_is_bad_input() {
    let d = validate_order_qty(0).expect("zero qty is meaningless — must be detected as bad input");
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::BadInput);
}

// ---------------------------------------------------------------------------
// 3. Positive quantities → valid
// ---------------------------------------------------------------------------

#[test]
fn positive_order_qty_passes_validation() {
    assert!(validate_order_qty(1).is_none(), "qty=1 must pass");
    assert!(validate_order_qty(100).is_none(), "qty=100 must pass");
    assert!(
        validate_order_qty(1_000_000).is_none(),
        "qty=1_000_000 must pass"
    );
}

// ---------------------------------------------------------------------------
// 4. i64::MIN quantity → bad input (extreme, must not panic)
// ---------------------------------------------------------------------------

#[test]
fn i64_min_order_qty_halts_without_panic() {
    let d = validate_order_qty(i64::MIN)
        .expect("i64::MIN qty must be detected as bad input without panicking");
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::BadInput);
}

// ---------------------------------------------------------------------------
// 5. i64::MAX quantity → valid (maximum representable positive)
// ---------------------------------------------------------------------------

#[test]
fn i64_max_order_qty_passes_validation() {
    assert!(
        validate_order_qty(i64::MAX).is_none(),
        "i64::MAX is positive — must pass validation"
    );
}

// ---------------------------------------------------------------------------
// 6. Halted engine rejects new orders — negative qty cannot bypass sticky halt
// ---------------------------------------------------------------------------

#[test]
fn halted_engine_rejects_new_order_regardless_of_qty_sign() {
    // Simulate a halt that was caused by a prior bad-input event.
    let cfg = RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 10,
        pdt_auto_enabled: false,
        missing_protective_stop_flattens: true,
    };
    let mut st = RiskState::new(20260101, 100_000 * M, 1);
    st.halted = true; // sticky: set from a prior bad-input event

    let inp = RiskInput {
        day_id: 20260101,
        equity_micros: 100_000 * M,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };

    let d = evaluate(&cfg, &mut st, &inp);
    assert_eq!(
        d.action,
        RiskAction::Reject,
        "halted engine must reject new orders"
    );
    assert_eq!(
        d.reason,
        ReasonCode::AlreadyHalted,
        "reason must be AlreadyHalted, not a bypass"
    );
}
