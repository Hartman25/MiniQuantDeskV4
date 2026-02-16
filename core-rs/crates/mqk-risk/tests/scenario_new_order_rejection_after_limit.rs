use mqk_risk::*;

const M: i64 = 1_000_000;

#[test]
fn scenario_new_order_rejection_after_limit_and_reject_storm_halts() {
    let cfg = RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 3,
        pdt_auto_enabled: true,
        missing_protective_stop_flattens: true,
    };

    let mut st = RiskState::new(20260216, 100_000 * M, 10);

    // Record 3 rejects in the same window => next evaluate should HALT due to storm.
    st.record_reject(10);
    st.record_reject(10);
    st.record_reject(10);

    let inp_halt = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 10,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };

    let d1 = evaluate(&cfg, &mut st, &inp_halt);
    assert_eq!(d1.action, RiskAction::Halt);
    assert_eq!(d1.reason, ReasonCode::RejectStormBreached);
    assert!(st.halted);

    // After halted, new orders are rejected.
    let inp_reject = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 10,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };

    let d2 = evaluate(&cfg, &mut st, &inp_reject);
    assert_eq!(d2.action, RiskAction::Reject);
    assert_eq!(d2.reason, ReasonCode::AlreadyHalted);

    // Flatten is still allowed.
    let inp_flatten = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 10,
        request: RequestKind::Flatten,
        is_risk_reducing: true,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };

    let d3 = evaluate(&cfg, &mut st, &inp_flatten);
    assert_eq!(d3.action, RiskAction::Allow);
    assert_eq!(d3.reason, ReasonCode::AlreadyHalted);
}
