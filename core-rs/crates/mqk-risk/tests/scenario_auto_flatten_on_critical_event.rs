use mqk_risk::*;

const M: i64 = 1_000_000;

#[test]
fn scenario_auto_flatten_on_missing_protective_stop_kill_switch() {
    let cfg = RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 10,
        pdt_auto_enabled: true,
        missing_protective_stop_flattens: true,
    };

    let mut st = RiskState::new(20260216, 100_000 * M, 5);

    let ks = KillSwitchEvent::new(KillSwitchType::MissingProtectiveStop)
        .with_evidence("symbol", "AAPL")
        .with_evidence("order_id", "missing");

    let inp = RiskInput {
        day_id: 20260216,
        equity_micros: 100_000 * M,
        reject_window_id: 5,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: Some(ks.clone()),
    };

    let d = evaluate(&cfg, &mut st, &inp);

    assert_eq!(d.action, RiskAction::FlattenAndHalt);
    assert_eq!(d.reason, ReasonCode::KillSwitchTriggered);

    let got = d.kill_switch.expect("kill switch attached");
    assert_eq!(got.kind, KillSwitchType::MissingProtectiveStop);
    assert_eq!(got.code, "KILL_SWITCH_MISSING_PROTECTIVE_STOP");
    assert!(st.halted);
    assert!(st.disarmed);
}
