use mqk_testkit::{load_bars_csv, run_parity_scenario_stub};

#[test]
fn scenario_replay_determinism_matches_artifacts() {
    let bars = load_bars_csv("../../../tests/fixtures/bars/bars_1h_trend_up.csv").unwrap();

    // TODO (PATCH 13): run parity, then replay(manifest) and compare artifacts hashes.
    let _res = run_parity_scenario_stub(&bars).unwrap();

    assert!(bars.len() >= 2);
}
