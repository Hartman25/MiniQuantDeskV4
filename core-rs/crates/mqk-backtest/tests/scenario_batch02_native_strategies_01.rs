//! The Batch 02 native engines, driven through the real BacktestEngine window and the native
//! signal emitter on real-calendar daily bars (00:00 ET labels), must equal an independent
//! reference at every bar, declare their true history requirement, bind the emitter fingerprint
//! to the registered engine, and size identically through the ONE capital-fraction wrapper.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use mqk_backtest::{
    emit_native_signal_stream, BacktestBar, BacktestConfig, BacktestEngine, SizingPolicy,
};
use mqk_integrity::sessions;
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, Strategy, StrategyContext, StrategySpec,
};
use std::collections::BTreeMap;

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;

fn label(date: NaiveDate) -> i64 {
    New_York
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp()
}

/// Every regular session from 2022-01-03 through 2024-12-31 (spans Good Friday, Juneteenth,
/// half-days, a leap February, the 2022-2023 year boundary and the 2023-2024 one).
fn session_dates() -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut x = NaiveDate::from_ymd_opt(2022, 1, 3).unwrap();
    let end = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
    while x <= end {
        if sessions::is_session(x).unwrap() {
            out.push(x);
        }
        x = x.succ_opt().unwrap();
    }
    out
}

fn closes(n: usize) -> Vec<i64> {
    (0..n)
        .map(|i| {
            let swing = ((i as f64 / 37.0).sin() * 9_000_000.0) as i64;
            let noise = (((i * 37) % 23) as i64 - 11) * 120_000;
            140_000_000 + i as i64 * 40_000 + swing + noise
        })
        .collect()
}

fn bars(dates: &[NaiveDate], c: &[i64]) -> Vec<BacktestBar> {
    dates
        .iter()
        .zip(c)
        .map(|(d, &c)| {
            let mut bar = BacktestBar::new("SPY", label(*d), c, c + 500_000, c - 500_000, c, 1_000);
            // The CSV loader derives day_id from end_ts (YYYYMMDD); the constructor's fixed
            // default would freeze the risk day for the whole run.
            bar.day_id = (d.year() * 10_000) as u32 + d.month() * 100 + d.day();
            bar
        })
        .collect()
}

fn cfg() -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c
}

fn cf_cfg(bps: i64) -> BacktestConfig {
    let mut c = cfg();
    c.initial_cash_micros = CAPITAL;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(bps).unwrap();
    c
}

fn registered(name: &str) -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(name).unwrap()
}

/// Independent reference: group the session list by calendar month; the next session `N` is
/// long for turn-of-month iff it is among the first three or the last session of its month,
/// and long for Halloween iff its month is November..=April.
fn reference(name: &str, dates: &[NaiveDate], t: usize, all: &[NaiveDate]) -> i64 {
    // N(t) comes from the FULL covered session list, so the final bar still has a successor.
    let pos = all.iter().position(|d| *d == dates[t]).unwrap();
    let Some(n) = all.get(pos + 1) else { return 0 };
    let mut by_month: BTreeMap<(i32, u32), Vec<NaiveDate>> = BTreeMap::new();
    for d in all {
        by_month.entry((d.year(), d.month())).or_default().push(*d);
    }
    match name {
        "turn_of_month_last1_first3" => {
            let m = &by_month[&(n.year(), n.month())];
            let idx = m.iter().position(|d| d == n).unwrap();
            i64::from(idx < 3 || idx == m.len() - 1)
        }
        "halloween_nov_apr" => i64::from(matches!(n.month(), 11 | 12 | 1 | 2 | 3 | 4)),
        other => panic!("unknown engine {other}"),
    }
}

fn full_calendar() -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut x = sessions::coverage_start();
    while x <= sessions::coverage_end() {
        if sessions::is_session(x).unwrap() {
            out.push(x);
        }
        x = x.succ_opt().unwrap();
    }
    out
}

const ENGINES: [(&str, usize); 2] = [("turn_of_month_last1_first3", 1), ("halloween_nov_apr", 1)];

#[test]
fn emitted_stream_equals_the_independent_reference_at_every_bar() {
    let dates = session_dates();
    let all = full_calendar();
    let c = closes(dates.len());
    let b = bars(&dates, &c);
    for (name, required) in ENGINES {
        assert_eq!(registered(name).required_history_bars(), required, "{name}");
        let stream = emit_native_signal_stream(cfg(), &b, registered(name)).unwrap();
        assert_eq!(stream.rows.len(), dates.len(), "{name}");
        let got: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
        let expect: Vec<i64> = (0..dates.len())
            .map(|t| reference(name, &dates, t, &all) * M)
            .collect();
        assert_eq!(got, expect, "{name}: emitter vs reference");
        assert!(got.iter().any(|&q| q > 0) && got.contains(&0), "{name}");
        assert_eq!(
            stream.semantic_fingerprint,
            registered(name).semantic_fingerprint(),
            "{name}"
        );
        assert_eq!(stream.required_history_bars, 1, "{name}");
    }
}

/// Stateless: a fresh instance fed only the latest bar reproduces the long-running instance.
#[test]
fn fresh_instance_with_only_the_latest_bar_matches_at_every_bar() {
    let dates = session_dates();
    let c = closes(dates.len());
    for (name, _) in ENGINES {
        let mut long_lived = registered(name);
        for t in 0..dates.len() {
            let lo = t.saturating_sub(60);
            let full: Vec<BarStub> = (lo..=t)
                .map(|i| BarStub::new(label(dates[i]), true, c[i], 1))
                .collect();
            let latest = vec![full.last().unwrap().clone()];
            let ctx_of = |w: Vec<BarStub>| {
                StrategyContext::new(DAY, 0, RecentBarsWindow::new(w.len().max(1), w))
            };
            let a = long_lived.on_bar(&ctx_of(full)).targets[0].qty.raw();
            let b = registered(name).on_bar(&ctx_of(latest)).targets[0]
                .qty
                .raw();
            assert_eq!(a, b, "{name} bar {t}");
        }
    }
}

#[test]
fn registry_metadata_declares_the_history_requirement_daily_timeframe_and_spec() {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    for (name, required) in ENGINES {
        let meta = reg
            .list()
            .into_iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name} not registered"))
            .clone();
        assert_eq!(
            meta.data_requirements.unwrap().minimum_completed_bars,
            required
        );
        assert_eq!(meta.timeframe_secs, DAY);
        assert_eq!(registered(name).spec(), StrategySpec::new(name, DAY));
    }
}

/// Mission sizing proof: raw +1 -> FixedInitialCapitalFractionV1 (1000 bps of USD 100k = USD
/// 10,000) -> Q = floor(budget / causal completed-bar close); Research emits the SAME absolute Q
/// the canonical Backtest executes; the half-exposure 500 bps stress resolves its own smaller Q.
#[test]
fn capital_fraction_sizing_is_identical_through_the_one_wrapper_for_every_engine() {
    let dates = session_dates();
    let c = closes(dates.len());
    let b = bars(&dates, &c);
    for (name, _) in ENGINES {
        let mut notional_ratio_checked = 0;
        let mut qs: BTreeMap<i64, (Vec<i64>, Vec<i64>)> = BTreeMap::new();
        for bps in [1_000, 500] {
            let stream = emit_native_signal_stream(cf_cfg(bps), &b, registered(name)).unwrap();
            let mut e = BacktestEngine::new(cf_cfg(bps));
            e.add_strategy(registered(name)).unwrap();
            let report = e.run(&b).unwrap();
            assert!(!report.halted, "{name}@{bps}: {:?}", report.halt_reason);
            assert_eq!(stream.rows.len(), dates.len(), "{name}@{bps}");
            // Research == Backtest identity and sizing provenance.
            assert_eq!(stream.run_id, report.run_id, "{name}@{bps}");
            assert_eq!(
                stream.semantic_fingerprint, report.strategy_semantic_fingerprint,
                "{name}@{bps}"
            );
            assert_eq!(
                stream.sizing_provenance, report.sizing_provenance,
                "{name}@{bps}"
            );
            let budget = CAPITAL * bps / 10_000;
            assert!(!report.sizing_provenance.entries.is_empty(), "{name}@{bps}");
            for e in &report.sizing_provenance.entries {
                assert_eq!(e.allocation_fraction_bps, bps);
                assert_eq!(e.initial_allocated_capital_micros, CAPITAL);
                assert_eq!(e.position_budget_micros, budget);
                assert_eq!(
                    e.resolved_target_qty_micros,
                    (budget / e.causal_reference_price_micros) * M
                );
                assert_ne!(e.resolved_target_qty_micros, M, "never the raw +1 share");
            }
            // Every emitted positive target is an engine-resolved entry; held Q never resizes.
            let resolved: Vec<i64> = report
                .sizing_provenance
                .entries
                .iter()
                .map(|e| e.resolved_target_qty_micros)
                .collect();
            let targets: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
            assert!(targets
                .iter()
                .filter(|&&q| q > 0)
                .all(|q| resolved.contains(q)));
            qs.insert(bps, (resolved, targets));
        }
        let (full, full_targets) = &qs[&1_000];
        let (half, half_targets) = &qs[&500];
        // Same decisions (direction), different sizing: identical flat/long shape, stress Q <= baseline Q.
        assert_eq!(full.len(), half.len(), "{name}: same entries");
        assert_eq!(
            full_targets.iter().map(|q| *q > 0).collect::<Vec<_>>(),
            half_targets.iter().map(|q| *q > 0).collect::<Vec<_>>(),
            "{name}: stress changes sizing, never the decision"
        );
        for (f, h) in full.iter().zip(half) {
            assert!(h <= f, "{name}: stress Q must not exceed baseline Q");
            // Approximately half in notional, subject to whole-share flooring (never asserted as exact half the share count).
            assert!(
                *h * 2 + 2 * M >= *f && *h * 2 <= *f + M,
                "{name}: {h} vs {f}"
            );
            notional_ratio_checked += 1;
        }
        assert!(notional_ratio_checked > 0);
    }
}
