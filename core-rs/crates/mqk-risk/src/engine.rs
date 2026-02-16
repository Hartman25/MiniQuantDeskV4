use crate::{
    KillSwitchEvent, KillSwitchType, ReasonCode, RequestKind, RiskAction, RiskConfig, RiskDecision,
    RiskInput, RiskState,
};

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
    if cfg.daily_loss_limit_micros > 0 {
        let floor = st.day_start_equity_micros - cfg.daily_loss_limit_micros;
        if inp.equity_micros <= floor {
            st.halted = true;
            return RiskDecision {
                action: RiskAction::Halt,
                reason: ReasonCode::DailyLossLimitBreached,
                kill_switch: Some(
                    KillSwitchEvent::new(KillSwitchType::Manual)
                        .with_evidence("type", "DAILY_LOSS_LIMIT")
                        .with_evidence("day_start_equity_micros", st.day_start_equity_micros.to_string())
                        .with_evidence("equity_micros", inp.equity_micros.to_string())
                        .with_evidence("daily_loss_limit_micros", cfg.daily_loss_limit_micros.to_string()),
                ),
            };
        }
    }

    // 5) Max drawdown: flatten + halt when breached.
    if cfg.max_drawdown_limit_micros > 0 {
        let floor = st.peak_equity_micros - cfg.max_drawdown_limit_micros;
        if inp.equity_micros <= floor {
            st.halted = true;
            st.disarmed = true;
            return RiskDecision {
                action: RiskAction::FlattenAndHalt,
                reason: ReasonCode::MaxDrawdownBreached,
                kill_switch: Some(
                    KillSwitchEvent::new(KillSwitchType::Manual)
                        .with_evidence("type", "MAX_DRAWDOWN")
                        .with_evidence("peak_equity_micros", st.peak_equity_micros.to_string())
                        .with_evidence("equity_micros", inp.equity_micros.to_string())
                        .with_evidence("max_drawdown_limit_micros", cfg.max_drawdown_limit_micros.to_string()),
                ),
            };
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
                    .with_evidence("reject_count_in_window", st.reject_count_in_window.to_string())
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
