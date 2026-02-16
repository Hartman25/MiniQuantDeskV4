use mqk_testkit::{load_bars_csv, run_parity_scenario_stub};

#[test]
fn scenario_entry_places_protective_stop() {
    let bars = load_bars_csv("../../../tests/fixtures/bars/bars_1h_trend_up.csv").unwrap();

    // TODO (PATCH 08+10): run parity and assert order sequence:
    // - entry submitted
    // - protective stop submitted
    let _res = run_parity_scenario_stub(&bars).unwrap();

    // Placeholder assertion to keep test skeleton compiling once runner exists.
    assert!(!bars.is_empty());
}
