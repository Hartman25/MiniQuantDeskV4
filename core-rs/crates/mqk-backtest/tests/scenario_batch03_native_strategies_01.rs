//! The Batch 03 native engines (F01..F10), driven through the real BacktestEngine window and the
//! native signal emitter on real-calendar daily OHLC bars (00:00 ET labels), must equal an
//! independent integer reference at every bar, declare their true history requirement, bind the
//! emitter fingerprint to the registered engine, and size identically through the ONE
//! capital-fraction wrapper.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use mqk_backtest::{
    emit_native_signal_stream, BacktestBar, BacktestConfig, BacktestEngine, SizingPolicy,
};
use mqk_integrity::sessions;
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, RestartRecovery, Strategy, StrategyContext,
    StrategySpec,
};
use std::collections::BTreeMap;

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;
/// Fixture stress fraction exercised only to prove the sizing wrapper; it is NOT the Batch 03
/// stress contract (that decision is operator-owned and unmade).
const FIXTURE_STRESS_BPS: i64 = 500;

const F01: &str = "monthly_multihorizon_abs_momentum_consensus_v1";
const F02: &str = "trend_filtered_rsi5_reversion_v1";
const F03: &str = "trend_filtered_extreme_3d_atr_reversal_v1";
const F04: &str = "close_channel_100_50_trend_v1";
const F05: &str = "monthly_10month_trend_timing_v1";
const F06: &str = "trend_filtered_zscore20_reversion_v1";
const F07: &str = "volatility_contraction_breakout_v1";
const F08: &str = "monthly_12_minus_1_abs_momentum_v1";
const F09: &str = "delayed_overnight_gap_reversal_v1";
const F10: &str = "monthly_52week_high_proximity_v1";

const ENGINES: [(&str, usize); 10] = [
    (F01, 275),
    (F02, 200),
    (F03, 200),
    (F04, 101),
    (F05, 253),
    (F06, 200),
    (F07, 61),
    (F08, 275),
    (F09, 22),
    (F10, 274),
];
const STATELESS: [&str; 4] = [F01, F05, F08, F10];

fn label(date: NaiveDate) -> i64 {
    New_York
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp()
}

fn calendar(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut x = from;
    while x <= to {
        if sessions::is_session(x).unwrap() {
            out.push(x);
        }
        x = x.succ_opt().unwrap();
    }
    out
}

/// Every regular session 2019-01-02..2024-12-31 (holidays, half-days, leap days, year ends).
fn session_dates() -> Vec<NaiveDate> {
    calendar(
        NaiveDate::from_ymd_opt(2019, 1, 2).unwrap(),
        NaiveDate::from_ymd_opt(2024, 12, 31).unwrap(),
    )
}

fn full_calendar() -> Vec<NaiveDate> {
    calendar(sessions::coverage_start(), sessions::coverage_end())
}

#[derive(Clone)]
struct Ohlc {
    o: Vec<i64>,
    h: Vec<i64>,
    l: Vec<i64>,
    c: Vec<i64>,
}

/// Deterministic OHLC with volatility regimes (calm contractions after bursts), multi-day drops
/// and overnight gap-downs, so every family both enters and exits and F03/F07/F09 events occur.
fn series(n: usize) -> Ohlc {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) % 2_001) as i64 - 1_000
    };
    let (mut o, mut h, mut l, mut c) = (vec![], vec![], vec![], vec![]);
    let mut prev = 140 * M;
    for i in 0..n {
        let amp_bps: i64 = match (i / 45) % 4 {
            0 => 15,
            1 => 90,
            2 => 220,
            _ => 25,
        };
        let drift_bps: i64 = if (i / 180) % 3 != 2 { 45 } else { -10 };
        let mut open = prev + prev * (rnd() * amp_bps / 4 + drift_bps * 1_000) / 10_000_000;
        let mut step_bps = rnd() * amp_bps / 1_000;
        if i % 41 >= 36 {
            step_bps -= 160;
        }
        if i % 53 == 52 {
            open = prev - prev * 3 / 100;
        }
        let close = (open + open * step_bps / 10_000).max(M);
        let wick = open * (amp_bps / 2 + 3) / 10_000;
        o.push(open);
        h.push(open.max(close) + wick);
        l.push((open.min(close) - wick).max(1));
        c.push(close);
        prev = close;
    }
    Ohlc { o, h, l, c }
}

fn bars(dates: &[NaiveDate], s: &Ohlc) -> Vec<BacktestBar> {
    dates
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut bar = BacktestBar::new("SPY", label(*d), s.o[i], s.h[i], s.l[i], s.c[i], 1_000);
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

// ---------------------------------------------------------------------------------------------
// Independent integer references. Each is written from the predeclared family rule on the full
// OHLC history, not from the engine's window helpers.
// ---------------------------------------------------------------------------------------------

fn sum(v: &[i64]) -> i128 {
    v.iter().map(|&x| i128::from(x)).sum()
}

fn mean_below(c: &[i64], t: usize, n: usize) -> bool {
    n as i128 * i128::from(c[t]) > sum(&c[t + 1 - n..=t])
}

/// Latest month-end at or before `t`; a session is a month-end iff its successor session in the
/// FULL covered calendar falls in another month.
fn latest_month_end(dates: &[NaiveDate], all: &[NaiveDate], t: usize) -> Option<usize> {
    let is_end = |i: usize| {
        let pos = all.iter().position(|d| *d == dates[i]).unwrap();
        let next = all[pos + 1];
        (next.year(), next.month()) != (dates[i].year(), dates[i].month())
    };
    (0..=t).rev().find(|&i| is_end(i))
}

fn month_ends_before(dates: &[NaiveDate], all: &[NaiveDate], m: usize) -> Vec<usize> {
    (0..m)
        .filter(|&i| {
            let pos = all.iter().position(|d| *d == dates[i]).unwrap();
            let next = all[pos + 1];
            (next.year(), next.month()) != (dates[i].year(), dates[i].month())
        })
        .collect()
}

fn ref_f01(c: &[i64], me: Option<usize>, t: usize) -> bool {
    let Some(m) = me else { return false };
    if t + 1 < 275 {
        return false;
    }
    [21usize, 63, 252]
        .iter()
        .filter(|&&h| c[m] > c[m - h])
        .count()
        >= 2
}

fn ref_f05(c: &[i64], dates: &[NaiveDate], all: &[NaiveDate], me: Option<usize>, t: usize) -> bool {
    let Some(m) = me else { return false };
    if t + 1 < 253 {
        return false;
    }
    let prior = month_ends_before(dates, all, m);
    if prior.len() < 10 {
        return false;
    }
    let mean10: Vec<i64> = prior[prior.len() - 10..].iter().map(|&i| c[i]).collect();
    10 * i128::from(c[m]) > sum(&mean10)
}

fn ref_f08(c: &[i64], me: Option<usize>, t: usize) -> bool {
    let Some(m) = me else { return false };
    t + 1 >= 275 && m >= 252 && c[m - 21] > c[m - 252]
}

fn ref_f10(c: &[i64], me: Option<usize>, t: usize) -> bool {
    let Some(m) = me else { return false };
    if t + 1 < 274 || m < 251 {
        return false;
    }
    let high = *c[m - 251..=m].iter().max().unwrap();
    i128::from(c[m]) * 100 >= i128::from(high) * 95
}

fn true_range_sum_prior(s: &Ohlc, t: usize) -> i128 {
    (t - 20..t)
        .map(|i| {
            let pc = i128::from(s.c[i - 1]);
            let (h, l) = (i128::from(s.h[i]), i128::from(s.l[i]));
            (h - l).max((h - pc).abs()).max((l - pc).abs())
        })
        .sum()
}

fn ref_f02(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let mut long = false;
    (0..c.len())
        .map(|t| {
            if t + 1 < 200 {
                long = false;
                return false;
            }
            let trend = mean_below(c, t, 200);
            let (mut g, mut l) = (0i128, 0i128);
            for i in t - 4..=t {
                let d = i128::from(c[i]) - i128::from(c[i - 1]);
                if d > 0 {
                    g += d;
                } else {
                    l -= d;
                }
            }
            let oversold = g + l > 0 && 100 * g < 30 * (g + l);
            let overbought = g + l > 0 && 100 * g > 70 * (g + l);
            long = if long {
                !overbought && trend
            } else {
                trend && oversold
            };
            long
        })
        .collect()
}

fn ref_f03(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let mut held = 0u8;
    (0..c.len())
        .map(|t| {
            if t + 1 < 200 {
                held = 0;
                return false;
            }
            held = if (1..5).contains(&held) {
                held + 1
            } else {
                // drop_fraction > 1.5 * atr_fraction, cross-multiplied exactly
                let drop = (
                    i128::from(c[t - 3]) - i128::from(c[t]),
                    i128::from(c[t - 3]),
                );
                let atr = (
                    3 * true_range_sum_prior(s, t),
                    2 * 20 * i128::from(c[t - 1]),
                );
                u8::from(mean_below(c, t, 200) && drop.0 * atr.1 > atr.0 * drop.1)
            };
            held > 0
        })
        .collect()
}

fn ref_f04(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let mut long = false;
    (0..c.len())
        .map(|t| {
            if t < 100 {
                long = false;
                return false;
            }
            long = if long {
                c[t] >= *c[t - 50..t].iter().min().unwrap()
            } else {
                c[t] > *c[t - 100..t].iter().max().unwrap()
            };
            long
        })
        .collect()
}

fn ref_f06(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let mut long = false;
    (0..c.len())
        .map(|t| {
            if t + 1 < 200 {
                long = false;
                return false;
            }
            let prior = &c[t - 20..t];
            let n = 20i128;
            let s1 = sum(prior);
            // n * population variance numerator, exact: sum((n*x - s1)^2) / n
            let v = prior
                .iter()
                .map(|&x| (n * i128::from(x) - s1) * (n * i128::from(x) - s1))
                .sum::<i128>()
                / n;
            let d = s1 - n * i128::from(c[t]);
            let trend = mean_below(c, t, 200);
            long = if long {
                n * i128::from(c[t]) < s1 && trend
            } else {
                trend && v > 0 && d > 0 && d * d > 4 * v
            };
            long
        })
        .collect()
}

fn ref_f07(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let range = |t: usize, n: usize| -> i128 {
        i128::from(*s.h[t - n..t].iter().max().unwrap())
            - i128::from(*s.l[t - n..t].iter().min().unwrap())
    };
    let mut long = false;
    (0..c.len())
        .map(|t| {
            if t < 60 {
                long = false;
                return false;
            }
            long = if long {
                c[t] >= *c[t - 10..t].iter().min().unwrap()
            } else {
                let (r10, r60) = (range(t, 10), range(t, 60));
                r60 > 0 && r10 * 4 < r60 && c[t] > *c[t - 20..t].iter().max().unwrap()
            };
            long
        })
        .collect()
}

fn ref_f09(s: &Ohlc) -> Vec<bool> {
    let c = &s.c;
    let mut held = 0u8;
    (0..c.len())
        .map(|t| {
            if t + 1 < 22 {
                held = 0;
                return false;
            }
            held = if (1..3).contains(&held) {
                held + 1
            } else {
                let gap = i128::from(c[t - 1]) - i128::from(s.o[t]);
                u8::from(gap * 20 * 2 > 3 * true_range_sum_prior(s, t))
            };
            held > 0
        })
        .collect()
}

fn reference(name: &str, s: &Ohlc, dates: &[NaiveDate], all: &[NaiveDate]) -> Vec<bool> {
    let monthly = |f: &dyn Fn(Option<usize>, usize) -> bool| -> Vec<bool> {
        (0..dates.len())
            .map(|t| f(latest_month_end(dates, all, t), t))
            .collect()
    };
    match name {
        F01 => monthly(&|me, t| ref_f01(&s.c, me, t)),
        F05 => monthly(&|me, t| ref_f05(&s.c, dates, all, me, t)),
        F08 => monthly(&|me, t| ref_f08(&s.c, me, t)),
        F10 => monthly(&|me, t| ref_f10(&s.c, me, t)),
        F02 => ref_f02(s),
        F03 => ref_f03(s),
        F04 => ref_f04(s),
        F06 => ref_f06(s),
        F07 => ref_f07(s),
        F09 => ref_f09(s),
        other => panic!("unknown engine {other}"),
    }
}

#[test]
fn emitted_stream_equals_the_independent_reference_at_every_bar() {
    let dates = session_dates();
    let all = full_calendar();
    let s = series(dates.len());
    let b = bars(&dates, &s);
    for (name, required) in ENGINES {
        assert_eq!(registered(name).required_history_bars(), required, "{name}");
        let stream = emit_native_signal_stream(cfg(), &b, registered(name)).unwrap();
        assert_eq!(stream.rows.len(), dates.len(), "{name}");
        let got: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
        let expect: Vec<i64> = reference(name, &s, &dates, &all)
            .into_iter()
            .map(|long| i64::from(long) * M)
            .collect();
        assert_eq!(got, expect, "{name}: emitter vs reference");
        let longs = got.iter().filter(|&&q| q > 0).count();
        assert!(longs > 0 && longs < got.len(), "{name}: {longs} long bars");
        assert_eq!(
            stream.semantic_fingerprint,
            registered(name).semantic_fingerprint(),
            "{name}"
        );
        assert_eq!(stream.required_history_bars, required, "{name}");
        assert!(
            got.iter().take(required - 1).all(|&q| q == 0),
            "{name}: flat before the declared history exists"
        );
    }
}

/// The fixture must actually exercise the event families, not pass vacuously on all-flat output.
#[test]
fn the_fixture_fires_the_event_and_hold_families_and_every_exit_path() {
    let dates = session_dates();
    let s = series(dates.len());
    let all = full_calendar();
    let entries = |v: Vec<bool>| v.windows(2).filter(|w| !w[0] && w[1]).count();
    for (name, min_entries) in [(F02, 2), (F03, 2), (F04, 2), (F06, 1), (F07, 2), (F09, 3)] {
        let n = entries(reference(name, &s, &dates, &all));
        assert!(n >= min_entries, "{name}: only {n} entries");
    }
    for name in [F01, F05, F08, F10] {
        let r = reference(name, &s, &dates, &all);
        assert!(entries(r.clone()) >= 1 && r.iter().any(|&x| !x), "{name}");
    }
}

/// Stateless monthly engines: a fresh instance fed only its declared bounded history reproduces
/// the long-running instance. The stateful engines are deliberately NotRecoverable.
#[test]
fn fresh_instance_with_only_the_bounded_history_matches_for_stateless_engines() {
    let dates = session_dates();
    let s = series(dates.len());
    for (name, required) in ENGINES.into_iter().filter(|(n, _)| STATELESS.contains(n)) {
        let mut long_lived = registered(name);
        for t in 0..dates.len() {
            let lo = t.saturating_sub(300);
            let full: Vec<BarStub> = (lo..=t)
                .map(|i| BarStub::new(label(dates[i]), true, s.c[i], 1))
                .collect();
            let latest: Vec<BarStub> = full[full.len().saturating_sub(required)..].to_vec();
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
fn registry_metadata_declares_history_daily_timeframe_spec_and_restart_class() {
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
        let expected = if STATELESS.contains(&name) {
            RestartRecovery::BoundedHistoryReconstructible
        } else {
            RestartRecovery::NotRecoverable
        };
        assert_eq!(meta.restart_recovery, expected, "{name}");
    }
}

#[test]
fn every_family_has_a_distinct_symbol_bound_fingerprint() {
    let mut seen = BTreeMap::new();
    for (name, _) in ENGINES {
        let fp = registered(name).semantic_fingerprint();
        assert_eq!(fp.len(), 64, "{name}");
        assert!(
            seen.insert(fp, name).is_none(),
            "{name}: duplicate fingerprint"
        );
    }
}

/// raw +1 -> FixedInitialCapitalFractionV1 (1000 bps of USD 100k = USD 10,000) -> Q = floor(budget /
/// causal completed-bar close); Research emits the SAME absolute Q the canonical Backtest executes.
/// The fixture stress fraction resolves its own smaller Q and never changes a decision.
#[test]
fn capital_fraction_sizing_is_identical_through_the_one_wrapper_for_every_engine() {
    let dates = session_dates();
    let s = series(dates.len());
    let b = bars(&dates, &s);
    for (name, _) in ENGINES {
        let mut qs: BTreeMap<i64, (Vec<i64>, Vec<i64>)> = BTreeMap::new();
        for bps in [1_000, FIXTURE_STRESS_BPS] {
            let stream = emit_native_signal_stream(cf_cfg(bps), &b, registered(name)).unwrap();
            let mut e = BacktestEngine::new(cf_cfg(bps));
            e.add_strategy(registered(name)).unwrap();
            let report = e.run(&b).unwrap();
            assert!(!report.halted, "{name}@{bps}: {:?}", report.halt_reason);
            assert_eq!(stream.rows.len(), dates.len(), "{name}@{bps}");
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
        let (half, half_targets) = &qs[&FIXTURE_STRESS_BPS];
        assert_eq!(full.len(), half.len(), "{name}: same entries");
        assert_eq!(
            full_targets.iter().map(|q| *q > 0).collect::<Vec<_>>(),
            half_targets.iter().map(|q| *q > 0).collect::<Vec<_>>(),
            "{name}: stress changes sizing, never the decision"
        );
        for (f, h) in full.iter().zip(half) {
            assert!(h <= f, "{name}: stress Q must not exceed baseline Q");
            assert!(
                *h * 2 + 2 * M >= *f && *h * 2 <= *f + M,
                "{name}: {h} vs {f}"
            );
        }
    }
}
