use mqk_risk::*;

const M: i64 = 1_000_000;

#[test]
fn scenario_forced_halt_on_daily_loss_breach() {
    let cfg = RiskConfig {
        daily_loss_limit_micros: 1_000 * M, // $1,000
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 10,
        pdt_auto_enabled: true,
        missing_protective_stop_flattens: true,
    };

    // Start day at 100k
    let mut st = RiskState::new(20260216, 100_000 * M, 1);

    // Equity drops below 99k => breach => HALT
    let inp = RiskInput {
        day_id: 20260216,
        equity_micros: 98_900 * M,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    };

    let d = evaluate(&cfg, &mut st, &inp);
    assert_eq!(d.action, RiskAction::Halt);
    assert_eq!(d.reason, ReasonCode::DailyLossLimitBreached);
    assert!(st.halted);
}
