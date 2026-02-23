use crate::{
    KillSwitchEvent, KillSwitchType, ReasonCode, RequestKind, RiskAction, RiskConfig, RiskDecision,
    RiskInput, RiskState,
};

// ---------------------------------------------------------------------------
// Patch L10 — Exposure Sanity Clamps
// ---------------------------------------------------------------------------

/// Guard: `equity_micros` must be ≥ 0.
///
/// Negative equity is an unrepresentable value in this system (equity is always
/// non-negative).  If a bad upstream source produces a negative number, this
/// guard catches it *before* it can corrupt running state or cause arithmetic
/// overflow in the floor calculations below.
///
/// Returns `Some(Halt)` if the value is invalid; `None` if it passes.
pub fn validate_equity_input(equity_micros: i64) -> Option<RiskDecision> {
    if equity_micros < 0 {
        return Some(RiskDecision {
            action: RiskAction::Halt,
            reason: ReasonCode::BadInput,
            kill_switch: None,
        });
    }
    None
}

/// Guard: `order_qty` must be strictly positive (> 0).
///
/// A zero or negative order quantity has no meaningful interpretation as a new
/// order intent and is treated as bad input that halts deterministically.
///
/// Returns `Some(Halt)` if the value is invalid; `None` if it passes.
pub fn validate_order_qty(qty: i64) -> Option<RiskDecision> {
    if qty <= 0 {
        return Some(RiskDecision {
            action: RiskAction::Halt,
            reason: ReasonCode::BadInput,
            kill_switch: None,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Core engine
// ---------------------------------------------------------------------------

/// Deterministic tick maintenance:
/// - resets day_start_equity on day rollover
/// - updates peak equity
/// - resets reject counter on reject window rollover
pub fn tick(_cfg: &RiskConfig, st: &mut RiskState, inp: &RiskInput) {
    // Day rollover: reset day-start equity deterministically.
    if inp.day_id != st.day_id {
        st.day_id = inp.day_id;
        st.day_start_equity_micros = inp.equity_micros;
        // Peak is NOT reset here (max drawdown is lifetime peak during run).
        // If you want session-only peak later, that's a separate patch.
    }

    // Peak equity monotonic.
    if inp.equity_micros > st.peak_equity_micros {
        st.peak_equity_micros = inp.equity_micros;
    }

    // Reject window rollover.
    if inp.reject_window_id != st.reject_window_id {
        st.reject_window_id = inp.reject_window_id;
        st.reject_count_in_window = 0;
    }
}

/// Main evaluator (pure deterministic logic + sticky flags in state).
pub fn evaluate(cfg: &RiskConfig, st: &mut RiskState, inp: &RiskInput) -> RiskDecision {
    // 0) Sanity clamp — runs BEFORE tick so bad equity cannot corrupt running state.
    //    Negative equity is geometrically impossible in this system; treat as bad input.
    if let Some(_bad) = validate_equity_input(inp.equity_micros) {
        st.halted = true;
        return RiskDecision {
            action: RiskAction::Halt,
            reason: ReasonCode::BadInput,
            kill_switch: None,
        };
    }

    tick(cfg, st, inp);

    // 1) Kill switch overrides everything (critical).
    if let Some(ks) = &inp.kill_switch {
        st.halted = true;
        st.disarmed = true;

        let action = match ks.kind {
            KillSwitchType::MissingProtectiveStop => {
                if cfg.missing_protective_stop_flattens {
                    RiskAction::FlattenAndHalt
                } else {
                    RiskAction::Halt
                }
            }
            _ => RiskAction::FlattenAndHalt,
        };

        return RiskDecision {
            action,
            reason: ReasonCode::KillSwitchTriggered,
            kill_switch: Some(ks.clone()),
        };
    }

    // 2) Sticky halt behavior:
    // Once halted, reject anything that isn't flatten (flatten is allowed to reduce risk).
    if st.halted {
        return match inp.request {
            RequestKind::Flatten => RiskDecision {
                action: RiskAction::Allow,
                reason: ReasonCode::AlreadyHalted,
                kill_switch: None,
            },
            _ => RiskDecision {
                action: RiskAction::Reject,
                reason: ReasonCode::AlreadyHalted,
                kill_switch: None,
            },
        };
    }

    // 3) PDT auto enforcement (block new risk when pdt_ok == false).
    if cfg.pdt_auto_enabled && !inp.pdt.pdt_ok && !inp.is_risk_reducing {
        return RiskDecision {
            action: RiskAction::Reject,
            reason: ReasonCode::PdtPrevented,
            kill_switch: Some(
                KillSwitchEvent::new(KillSwitchType::Manual)
                    .with_evidence("type", "PDT_PREVENTED")
                    .with_evidence("pdt_ok", "false"),
            ),
        };
    }

    // 4) Daily loss limit: halt trading when breached.
    //    Use checked_sub to guard against corrupted state where day_start_equity_micros
    //    is extreme-negative — overflow would produce a wrong floor that masks the breach.
    if cfg.daily_loss_limit_micros > 0 {
        match st
            .day_start_equity_micros
            .checked_sub(cfg.daily_loss_limit_micros)
        {
            None => {
                // Arithmetic underflow: day_start_equity is corrupted or limit is absurd.
                // Fail-closed: halt rather than risk silently skipping the check.
                st.halted = true;
                return RiskDecision {
                    action: RiskAction::Halt,
                    reason: ReasonCode::BadInput,
                    kill_switch: None,
                };
            }
            Some(floor) => {
                if inp.equity_micros <= floor {
                    st.halted = true;
                    return RiskDecision {
                        action: RiskAction::Halt,
                        reason: ReasonCode::DailyLossLimitBreached,
                        kill_switch: Some(
                            KillSwitchEvent::new(KillSwitchType::Manual)
                                .with_evidence("type", "DAILY_LOSS_LIMIT")
                                .with_evidence(
                                    "day_start_equity_micros",
                                    st.day_start_equity_micros.to_string(),
                                )
                                .with_evidence("equity_micros", inp.equity_micros.to_string())
                                .with_evidence(
                                    "daily_loss_limit_micros",
                                    cfg.daily_loss_limit_micros.to_string(),
                                ),
                        ),
                    };
                }
            }
        }
    }

    // 5) Max drawdown: flatten + halt when breached.
    //    checked_sub is belt-and-suspenders here: in steady state peak_equity_micros is
    //    always ≥ 0 (equity guard above prevents negative equity from reaching tick), so
    //    underflow cannot occur in practice.  We use checked_sub anyway for defence-in-depth.
    if cfg.max_drawdown_limit_micros > 0 {
        match st
            .peak_equity_micros
            .checked_sub(cfg.max_drawdown_limit_micros)
        {
            None => {
                st.halted = true;
                st.disarmed = true;
                return RiskDecision {
                    action: RiskAction::Halt,
                    reason: ReasonCode::BadInput,
                    kill_switch: None,
                };
            }
            Some(floor) => {
                if inp.equity_micros <= floor {
                    st.halted = true;
                    st.disarmed = true;
                    return RiskDecision {
                        action: RiskAction::FlattenAndHalt,
                        reason: ReasonCode::MaxDrawdownBreached,
                        kill_switch: Some(
                            KillSwitchEvent::new(KillSwitchType::Manual)
                                .with_evidence("type", "MAX_DRAWDOWN")
                                .with_evidence(
                                    "peak_equity_micros",
                                    st.peak_equity_micros.to_string(),
                                )
                                .with_evidence("equity_micros", inp.equity_micros.to_string())
                                .with_evidence(
                                    "max_drawdown_limit_micros",
                                    cfg.max_drawdown_limit_micros.to_string(),
                                ),
                        ),
                    };
                }
            }
        }
    }

    // 6) Reject storm: if already exceeded threshold in this window, halt.
    if st.reject_count_in_window >= cfg.reject_storm_max_rejects_in_window {
        st.halted = true;
        return RiskDecision {
            action: RiskAction::Halt,
            reason: ReasonCode::RejectStormBreached,
            kill_switch: Some(
                KillSwitchEvent::new(KillSwitchType::RejectStorm)
                    .with_evidence("reject_window_id", st.reject_window_id.to_string())
                    .with_evidence(
                        "reject_count_in_window",
                        st.reject_count_in_window.to_string(),
                    )
                    .with_evidence(
                        "reject_storm_max_rejects_in_window",
                        cfg.reject_storm_max_rejects_in_window.to_string(),
                    ),
            ),
        };
    }

    // Otherwise allowed.
    RiskDecision {
        action: RiskAction::Allow,
        reason: ReasonCode::Allowed,
        kill_switch: None,
    }
}
