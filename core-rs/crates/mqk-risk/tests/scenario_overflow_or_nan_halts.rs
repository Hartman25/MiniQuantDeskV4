//! Scenario: Overflow or NaN-equivalent inputs halt the risk engine — Patch L10
//!
//! # Background
//!
//! With integer micros there is no IEEE-754 NaN, but arithmetic overflow is the
//! integer equivalent: `i64::MIN - positive_value` wraps or panics depending on
//! build profile.  We use `checked_sub` throughout the engine floor calculations
//! and add an early `validate_equity_input` guard so bad values are caught before
//! they can corrupt running state.
//!
//! # Invariants under test
//!
//! 1. `validate_equity_input(-1)`     → Halt / BadInput
//! 2. `validate_equity_input(i64::MIN)` → Halt / BadInput  (no panic)
//! 3. `validate_equity_input(0)`      → None   (zero is valid, not negative)
//! 4. `validate_equity_input(i64::MAX)` → None (large positive is valid)
//! 5. Corrupted `day_start_equity_micros = i64::MIN` in state causes daily-loss
//!    floor calculation to underflow → engine halts instead of panicking.
//! 6. Negative equity passed to `evaluate()` halts and sets the sticky flag.
//! 7. Equity guard runs BEFORE `tick()`: bad equity cannot corrupt `peak_equity_micros`.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_risk::*;

const M: i64 = 1_000_000; // 1 unit in micros

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn no_limit_cfg() -> RiskConfig {
    RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 10,
        pdt_auto_enabled: false,
        missing_protective_stop_flattens: true,
    }
}

fn cfg_with_daily_loss(limit_micros: i64) -> RiskConfig {
    RiskConfig {
        daily_loss_limit_micros: limit_micros,
        ..no_limit_cfg()
    }
}

fn inp_with_equity(equity_micros: i64) -> RiskInput {
    RiskInput {
        day_id: 20260101,
        equity_micros,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Negative equity → bad input
// ---------------------------------------------------------------------------

#[test]
fn negative_equity_micros_detected_by_validator() {
    let d = validate_equity_input(-1).expect("negative equity must be detected as bad input");
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::BadInput);
}

// ---------------------------------------------------------------------------
// 2. i64::MIN equity → bad input (extreme case, must not panic)
// ---------------------------------------------------------------------------

#[test]
fn i64_min_equity_detected_by_validator_without_panic() {
    let d = validate_equity_input(i64::MIN)
        .expect("i64::MIN equity must be detected as bad input without panicking");
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::BadInput);
}

// ---------------------------------------------------------------------------
// 3. Zero equity → valid (zero is not negative; no arithmetic hazard)
// ---------------------------------------------------------------------------

#[test]
fn zero_equity_passes_validator() {
    assert!(
        validate_equity_input(0).is_none(),
        "zero equity is not negative — validator must pass it through"
    );
}

// ---------------------------------------------------------------------------
// 4. Large positive equity → valid
// ---------------------------------------------------------------------------

#[test]
fn large_positive_equity_passes_validator() {
    assert!(validate_equity_input(1).is_none(), "equity=1 must pass");
    assert!(
        validate_equity_input(100_000 * M).is_none(),
        "equity=100_000*M must pass"
    );
    assert!(
        validate_equity_input(i64::MAX).is_none(),
        "equity=i64::MAX must pass"
    );
}

// ---------------------------------------------------------------------------
// 5. Corrupted day_start_equity_micros = i64::MIN → daily-loss floor underflows
//    → engine halts deterministically instead of panicking
// ---------------------------------------------------------------------------

#[test]
fn overflow_in_daily_loss_floor_halts_instead_of_panicking() {
    // Construct state with a pathologically bad day_start_equity_micros.
    // This simulates corrupted or extreme persisted state from a bad upstream source.
    // RiskState::new sets day_start_equity_micros = equity_micros = i64::MIN.
    let cfg = cfg_with_daily_loss(1_000 * M); // $1 000 loss limit
    let mut st = RiskState::new(20260101, i64::MIN, 1);

    // inp.equity_micros = 0 is valid (≥ 0), so the equity guard passes.
    // tick() keeps day_start_equity_micros = i64::MIN (same day_id, no rollover).
    // Floor = i64::MIN.checked_sub(1_000*M) = None → halt instead of wrapping.
    let inp = inp_with_equity(0);
    let d = evaluate(&cfg, &mut st, &inp);

    assert_eq!(
        d.action,
        RiskAction::Halt,
        "arithmetic underflow in daily-loss floor must halt, not panic"
    );
    assert!(
        st.halted,
        "halted flag must be set on overflow-triggered halt"
    );
}

// ---------------------------------------------------------------------------
// 6. Negative equity in evaluate() halts and sets the sticky flag
// ---------------------------------------------------------------------------

#[test]
fn negative_equity_in_evaluate_halts_and_is_sticky() {
    let cfg = no_limit_cfg();
    let mut st = RiskState::new(20260101, 100_000 * M, 1);

    let d = evaluate(&cfg, &mut st, &inp_with_equity(-500 * M));

    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(
        d.reason,
        ReasonCode::BadInput,
        "negative equity must be reported as bad input"
    );
    assert!(
        st.halted,
        "sticky halt flag must be set after bad-input detection"
    );
}

// ---------------------------------------------------------------------------
// 7. Equity guard runs BEFORE tick: bad equity cannot corrupt peak_equity_micros
// ---------------------------------------------------------------------------

#[test]
fn equity_guard_prevents_peak_equity_corruption() {
    let cfg = no_limit_cfg();
    let mut st = RiskState::new(20260101, 100_000 * M, 1);
    let original_peak = st.peak_equity_micros;

    // A bad negative equity is caught before tick() can update peak_equity_micros.
    let _ = evaluate(&cfg, &mut st, &inp_with_equity(-1));

    assert_eq!(
        st.peak_equity_micros, original_peak,
        "peak_equity_micros must be unchanged when bad equity is caught early"
    );
}
