#![forbid(unsafe_code)]
#![cfg(any(test, feature = "testkit"))]

use std::sync::{Arc, Mutex};

use chrono::{Datelike, Utc};
use mqk_broker_paper::LockedPaperBroker;
use mqk_execution::gateway::{BrokerGateway, IntegrityGate, ReconcileGate, RiskGate};
use mqk_execution::wiring::build_gateway;
use mqk_risk::{
    evaluate, PdtContext, ReasonCode, RequestKind, RiskAction, RiskConfig, RiskInput, RiskState,
};

#[derive(Clone, Copy)]
pub struct PassGate;

impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}

impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct RuntimeRiskGate {
    state: Arc<Mutex<RuntimeRiskGateState>>,
}

enum RuntimeRiskGateState {
    Ready {
        config: RiskConfig,
        state: RiskState,
        input: RiskInput,
        auto_time_context: bool,
    },
    FailClosed {
        denial: mqk_execution::RiskDenial,
    },
}

impl RuntimeRiskGate {
    pub fn from_run_config(config_json: &serde_json::Value, initial_equity_micros: i64) -> Self {
        match runtime_risk_config_from_run_config(config_json, initial_equity_micros) {
            Ok(config) => {
                let now = Utc::now();
                let day_id = runtime_day_id(now);
                let reject_window_id = runtime_reject_window_id(now);
                Self::ready(
                    config,
                    RiskState::new(day_id, initial_equity_micros, reject_window_id),
                    RiskInput {
                        day_id,
                        equity_micros: initial_equity_micros,
                        reject_window_id,
                        request: RequestKind::NewOrder,
                        is_risk_reducing: false,
                        pdt: PdtContext::ok(),
                        kill_switch: None,
                    },
                    true,
                )
            }
            Err(denial) => Self::fail_closed(denial),
        }
    }

    pub fn for_test(config: RiskConfig, state: RiskState, input: RiskInput) -> Self {
        if input.equity_micros <= 0 {
            return Self::fail_closed(runtime_risk_fail_closed_denial());
        }
        Self::ready(config, state, input, false)
    }

    fn ready(
        config: RiskConfig,
        state: RiskState,
        input: RiskInput,
        auto_time_context: bool,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeRiskGateState::Ready {
                config,
                state,
                input,
                auto_time_context,
            })),
        }
    }

    fn fail_closed(denial: mqk_execution::RiskDenial) -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeRiskGateState::FailClosed { denial })),
        }
    }
}

impl RiskGate for RuntimeRiskGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        let mut state = self.state.lock().expect("runtime risk gate lock");
        match &mut *state {
            RuntimeRiskGateState::FailClosed { denial } => {
                mqk_execution::RiskDecision::Deny(denial.clone())
            }
            RuntimeRiskGateState::Ready {
                config,
                state,
                input,
                auto_time_context,
            } => {
                if input.equity_micros <= 0 {
                    return mqk_execution::RiskDecision::Deny(runtime_risk_fail_closed_denial());
                }
                if *auto_time_context {
                    let now = Utc::now();
                    input.day_id = runtime_day_id(now);
                    input.reject_window_id = runtime_reject_window_id(now);
                }
                let decision = evaluate(config, state, input);
                runtime_risk_decision_to_execution_decision(config, &decision)
            }
        }
    }
}

fn runtime_risk_config_from_run_config(
    config_json: &serde_json::Value,
    initial_equity_micros: i64,
) -> Result<RiskConfig, mqk_execution::RiskDenial> {
    if initial_equity_micros <= 0 {
        return Err(runtime_risk_fail_closed_denial());
    }

    let defaults = RiskConfig::sane_defaults();
    let daily_loss_ratio = config_json
        .pointer("/risk/daily_loss_limit")
        .and_then(|value| value.as_f64())
        .ok_or_else(runtime_risk_fail_closed_denial)?;

    let daily_loss_limit_micros = ratio_limit_to_micros(daily_loss_ratio, initial_equity_micros)
        .ok_or_else(runtime_risk_fail_closed_denial)?;

    let max_drawdown_limit_micros = match config_json
        .pointer("/risk/max_drawdown")
        .and_then(|value| value.as_f64())
    {
        Some(ratio) => ratio_limit_to_micros(ratio, initial_equity_micros)
            .ok_or_else(runtime_risk_fail_closed_denial)?,
        None => 0,
    };

    let reject_storm_max_rejects_in_window = match config_json
        .pointer("/risk/reject_storm/max_rejects")
        .and_then(|value| value.as_i64())
    {
        Some(value) if value > 0 => value as u32,
        Some(_) => return Err(runtime_risk_fail_closed_denial()),
        None => defaults.reject_storm_max_rejects_in_window,
    };

    Ok(RiskConfig {
        daily_loss_limit_micros,
        max_drawdown_limit_micros,
        reject_storm_max_rejects_in_window,
        pdt_auto_enabled: config_json
            .pointer("/risk/pdt_auto_enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(defaults.pdt_auto_enabled),
        missing_protective_stop_flattens: config_json
            .pointer("/risk/missing_protective_stop_flattens")
            .and_then(|value| value.as_bool())
            .unwrap_or(defaults.missing_protective_stop_flattens),
    })
}

fn ratio_limit_to_micros(ratio: f64, initial_equity_micros: i64) -> Option<i64> {
    if !ratio.is_finite() || ratio <= 0.0 || initial_equity_micros <= 0 {
        return None;
    }

    let limit = ratio * initial_equity_micros as f64;
    if !limit.is_finite() || limit <= 0.0 || limit > i64::MAX as f64 {
        return None;
    }

    Some(limit.round() as i64)
}

fn runtime_day_id(now: chrono::DateTime<chrono::Utc>) -> u32 {
    let year = now.year() as u32;
    year * 10_000 + now.month() * 100 + now.day()
}

fn runtime_reject_window_id(now: chrono::DateTime<chrono::Utc>) -> u32 {
    let bucket = now.timestamp().div_euclid(60);
    bucket.clamp(0, u32::MAX as i64) as u32
}

fn runtime_risk_fail_closed_denial() -> mqk_execution::RiskDenial {
    mqk_execution::RiskDenial {
        reason: mqk_execution::RiskReason::RiskEngineUnavailable,
        evidence: mqk_execution::RiskEvidence::default(),
    }
}

fn runtime_risk_decision_to_execution_decision(
    config: &RiskConfig,
    decision: &mqk_risk::RiskDecision,
) -> mqk_execution::RiskDecision {
    match decision.action {
        RiskAction::Allow => mqk_execution::RiskDecision::Allow,
        _ => mqk_execution::RiskDecision::Deny(runtime_risk_engine_denial(config, decision)),
    }
}

fn runtime_risk_engine_denial(
    config: &RiskConfig,
    decision: &mqk_risk::RiskDecision,
) -> mqk_execution::RiskDenial {
    let mut evidence = mqk_execution::RiskEvidence::default();
    match decision.reason {
        ReasonCode::DailyLossLimitBreached => {
            evidence.limit = Some(config.daily_loss_limit_micros);
        }
        ReasonCode::MaxDrawdownBreached => {
            evidence.limit = Some(config.max_drawdown_limit_micros);
        }
        ReasonCode::RejectStormBreached => {
            evidence.limit = Some(config.reject_storm_max_rejects_in_window as i64);
        }
        _ => {}
    }

    mqk_execution::RiskDenial {
        reason: mqk_execution::RiskReason::RiskEngineUnavailable,
        evidence,
    }
}

/// TESTKIT ONLY.
/// This exists to run deterministic paper loops in validation harnesses.
/// MUST NOT be used by production binaries.
pub fn paper_gateway_for_testkit_validation(
    run_config_json: &serde_json::Value,
    initial_equity_micros: i64,
) -> BrokerGateway<LockedPaperBroker, PassGate, RuntimeRiskGate, PassGate> {
    build_gateway(
        LockedPaperBroker::new(),
        PassGate,
        RuntimeRiskGate::from_run_config(run_config_json, initial_equity_micros),
        PassGate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_execution::{BrokerSubmitRequest, GateRefusal, OutboxClaimToken, Side, SubmitError};

    fn make_claim() -> OutboxClaimToken {
        OutboxClaimToken::for_test(1, "paper-risk")
    }

    fn make_submit_req() -> BrokerSubmitRequest {
        BrokerSubmitRequest {
            order_id: "paper-risk".to_string(),
            symbol: "SPY".to_string(),
            side: Side::Buy,
            quantity: 10,
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
        }
    }

    #[test]
    fn runtime_risk_gate_blocks_submit_when_evaluate_denies() {
        let risk_gate = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 1_000 * 1_000_000,
                max_drawdown_limit_micros: 0,
                reject_storm_max_rejects_in_window: 10,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20240101, 100_000 * 1_000_000, 1),
            RiskInput {
                day_id: 20240101,
                equity_micros: 98_999 * 1_000_000,
                reject_window_id: 1,
                request: RequestKind::NewOrder,
                is_risk_reducing: false,
                pdt: PdtContext::ok(),
                kill_switch: None,
            },
        );

        let gateway = build_gateway(LockedPaperBroker::new(), PassGate, risk_gate, PassGate);
        let err = gateway
            .submit(&make_claim(), make_submit_req())
            .expect_err("real runtime risk gate must block denied submit");

        let SubmitError::Gate(GateRefusal::RiskBlocked(_)) = err else {
            panic!("expected SubmitError::Gate(RiskBlocked), got {err:?}");
        };
    }

    #[test]
    fn runtime_risk_gate_fails_closed_on_missing_or_ambiguous_input() {
        let risk_gate = RuntimeRiskGate::from_run_config(&serde_json::json!({}), 0);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("missing runtime risk inputs must deny");
        };
        assert_eq!(
            denial.reason,
            mqk_execution::RiskReason::RiskEngineUnavailable,
            "missing runtime risk inputs must fail closed"
        );
    }

    #[test]
    fn runtime_dispatch_path_uses_real_risk_adapter() {
        let gateway =
            paper_gateway_for_testkit_validation(&serde_json::json!({}), 100_000 * 1_000_000);
        let err = gateway
            .submit(&make_claim(), make_submit_req())
            .expect_err("paper runtime wiring must use the real runtime risk adapter");

        let SubmitError::Gate(GateRefusal::RiskBlocked(_)) = err else {
            panic!("expected SubmitError::Gate(RiskBlocked), got {err:?}");
        };
    }
}
