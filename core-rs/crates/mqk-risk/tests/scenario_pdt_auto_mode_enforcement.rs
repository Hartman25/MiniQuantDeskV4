use mqk_risk::*;

const M: i64 = 1_000_000;

#[test]
fn scenario_pdt_auto_mode_blocks_new_risk_allows_risk_reducing() {
    let cfg = RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 10,
        pdt_auto_enabled: true,
        missing_protective_stop_flattens: true,
    };

    let mut st = RiskState::new(20260216, 100_000 * M, 1);

    // New risk blocked.
    let inp_block = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::blocked(),
        kill_switch: None,
    };

    let d1 = evaluate(&cfg, &mut st, &inp_block);
    assert_eq!(d1.action, RiskAction::Reject);
    assert_eq!(d1.reason, ReasonCode::PdtPrevented);

    // Risk reducing allowed.
    let inp_reduce = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 1,
        request: RequestKind::Flatten,
        is_risk_reducing: true,
        pdt: PdtContext::blocked(),
        kill_switch: None,
    };

    let d2 = evaluate(&cfg, &mut st, &inp_reduce);
    assert_eq!(d2.action, RiskAction::Allow);
    assert_eq!(d2.reason, ReasonCode::Allowed);
}
