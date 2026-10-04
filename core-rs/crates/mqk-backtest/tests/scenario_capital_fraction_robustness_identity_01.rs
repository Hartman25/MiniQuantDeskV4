//! The Rust robustness gauntlet and native stress suite must test a capital-fraction candidate
//! as the candidate it is. Before this seam was repaired each scenario compared the UNWRAPPED
//! factory instance's fingerprint with the baseline's capital-fraction WRAPPER fingerprint, which
//! can never match, so every scenario failed with a fingerprint mismatch and no capital-fraction
//! candidate could ever pass robustness.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use mqk_backtest::{
    run_backtest_stress_suite, run_robustness_gauntlet, BacktestBar, BacktestConfig,
    BacktestEngine, SizingPolicy,
};
use mqk_integrity::sessions;
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy};

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;
const ENGINE: &str = "halloween_nov_apr";

fn label(date: NaiveDate) -> i64 {
    New_York
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp()
}

fn bars() -> Vec<BacktestBar> {
    let mut out = Vec::new();
    let mut x = NaiveDate::from_ymd_opt(2022, 1, 3).unwrap();
    let end = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
    let mut i = 0i64;
    while x <= end {
        if sessions::is_session(x).unwrap() {
            let c = 140 * M + i * 30_000 + (((i * 37) % 23) - 11) * 90_000;
            let mut b =
                BacktestBar::new("SPY", label(x), c, c + 300_000, c - 300_000, c, 5_000_000);
            b.day_id = (x.year() * 10_000) as u32 + x.month() * 100 + x.day();
            out.push(b);
            i += 1;
        }
        x = x.succ_opt().unwrap();
    }
    out
}

fn cfg(bps: i64) -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c.initial_cash_micros = CAPITAL;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(bps).unwrap();
    c
}

fn make(name: &'static str, symbol: &'static str) -> impl Fn() -> Box<dyn Strategy> {
    move || {
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut reg, symbol, 1, None, None).unwrap();
        reg.instantiate(name).unwrap()
    }
}

fn baseline(bps: i64) -> mqk_backtest::BacktestReport {
    let mut e = BacktestEngine::new(cfg(bps));
    e.add_strategy(make(ENGINE, "SPY")()).unwrap();
    let r = e.run(&bars()).unwrap();
    assert!(!r.halted, "{:?}", r.halt_reason);
    r
}

fn mismatches(reasons: impl Iterator<Item = Option<String>>) -> Vec<String> {
    reasons
        .flatten()
        .filter(|r| r.contains("semantic fingerprint mismatch"))
        .collect()
}

#[test]
fn gauntlet_and_stress_suite_evaluate_the_capital_fraction_candidate_without_an_identity_mismatch()
{
    let report = baseline(1_000);
    let bars = bars();
    let g = run_robustness_gauntlet(&report, &cfg(1_000), &bars, make(ENGINE, "SPY"));
    let bad = mismatches(g.scenarios.iter().map(|s| s.reason.clone()));
    assert!(bad.is_empty(), "gauntlet identity mismatches: {bad:?}");
    // The identity-bound scenarios genuinely executed the candidate (evidence of a real run is the
    // absence of a fail-closed identity refusal AND a reason-free pass or an economic reason).
    for name in [
        "execution_delay_stress",
        "placebo_temporal_offset",
        "conservative_capacity_stress",
        "parameter_neighborhood_execution",
    ] {
        let s = g
            .scenarios
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} missing"));
        assert!(s.applicable, "{name}");
    }
    let s = run_backtest_stress_suite(&report, &cfg(1_000), &bars, make(ENGINE, "SPY"));
    let bad = mismatches(s.scenarios.iter().map(|x| x.reason.clone()));
    assert!(bad.is_empty(), "stress-suite identity mismatches: {bad:?}");
    assert!(
        s.scenarios.iter().all(|x| x.final_equity_micros > 0),
        "every stress scenario actually ran"
    );
}

#[test]
fn a_different_candidate_or_a_different_fraction_is_still_refused() {
    let report = baseline(1_000);
    let bars = bars();
    // Another engine posing as the baseline candidate.
    let wrong_engine = make("turn_of_month_last1_first3", "SPY");
    let g = run_robustness_gauntlet(&report, &cfg(1_000), &bars, wrong_engine);
    assert!(!mismatches(g.scenarios.iter().map(|s| s.reason.clone())).is_empty());
    let s = run_backtest_stress_suite(
        &report,
        &cfg(1_000),
        &bars,
        make("turn_of_month_last1_first3", "SPY"),
    );
    assert!(s.scenarios.iter().all(|x| !x.passed));
    // The same engine for another symbol (symbol is part of the identity).
    let g = run_robustness_gauntlet(&report, &cfg(1_000), &bars, make(ENGINE, "EFA"));
    assert!(!mismatches(g.scenarios.iter().map(|s| s.reason.clone())).is_empty());
    // The baseline report was produced at 1000 bps; claiming 500 bps in the base config is refused.
    let g = run_robustness_gauntlet(&report, &cfg(500), &bars, make(ENGINE, "SPY"));
    assert!(!mismatches(g.scenarios.iter().map(|s| s.reason.clone())).is_empty());
    let s = run_backtest_stress_suite(&report, &cfg(500), &bars, make(ENGINE, "SPY"));
    assert!(s.scenarios.iter().all(|x| !x.passed));
}
