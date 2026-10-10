//! EXT-032 `pre_holiday_two_session_long_v1`, driven through the real BacktestEngine and the
//! native signal emitter on real-calendar (v2) daily bars with capital-fraction sizing.
//!
//! Proves the causal timeline (signal bar != fill bar, entry inside the first window session,
//! exit on the reopening session), the exact whole-share quantity, fills priced from the fill
//! bar's range plus slippage, commission arithmetic, missing-bar behaviour, restart equivalence
//! and the absence of any phantom position after a restart.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use mqk_backtest::{
    emit_native_signal_stream, BacktestBar, BacktestConfig, BacktestEngine, SizingPolicy,
    StressProfile,
};
use mqk_integrity::sessions_v2;
use mqk_portfolio::Side;
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{
    BarStub, CapitalFractionSizedStrategy, HeldSizingRecord, HeldSizingScope, HeldSizingStatus,
    PluginRegistry, RecentBarsWindow, RestartRecovery, Strategy, StrategyContext, TargetSizing,
    HELD_SIZING_STATE_VERSION, SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
};
use std::collections::BTreeSet;

const NAME: &str = "pre_holiday_two_session_long_v1";
const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;
const BPS: i64 = 1_000;
const SLIPPAGE_BPS: i64 = 5;
const COMMISSION_PER_SHARE_MICROS: i64 = 5_000;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn label(date: NaiveDate) -> i64 {
    New_York
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp()
}

/// Every actual v2 session from 2023-10-02 to 2025-01-31 (Thanksgiving, Christmas/New Year,
/// Good Friday, Juneteenth, Independence Day, a leap day and the 2025-01-09 surprise closure).
fn session_dates() -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut x = d(2023, 10, 2);
    while x <= d(2025, 1, 31) {
        if sessions_v2::is_session(x).unwrap() {
            out.push(x);
        }
        x = x.succ_opt().unwrap();
    }
    out
}

struct Ohlc {
    o: Vec<i64>,
    h: Vec<i64>,
    l: Vec<i64>,
    c: Vec<i64>,
}

/// Deterministic, non-degenerate prices (Q varies by entry; high/low/close/open all differ).
fn series(n: usize) -> Ohlc {
    let (mut o, mut h, mut l, mut c) = (vec![], vec![], vec![], vec![]);
    for i in 0..n {
        let close = 430_000_000 + (i as i64 % 61) * 1_300_000 + (i as i64 * 97 % 17) * 210_000;
        let open = close - 400_000 + (i as i64 % 5) * 150_000;
        o.push(open);
        h.push(close.max(open) + 1_200_000 + (i as i64 % 3) * 100_000);
        l.push(close.min(open) - 1_100_000 - (i as i64 % 4) * 100_000);
        c.push(close);
    }
    Ohlc { o, h, l, c }
}

fn bars(dates: &[NaiveDate], s: &Ohlc) -> Vec<BacktestBar> {
    dates
        .iter()
        .zip(0..)
        .map(|(date, i): (&NaiveDate, usize)| {
            let mut bar =
                BacktestBar::new("SPY", label(*date), s.o[i], s.h[i], s.l[i], s.c[i], 1_000);
            bar.day_id = (date.year() * 10_000) as u32 + date.month() * 100 + date.day();
            bar
        })
        .collect()
}

fn cfg() -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c.initial_cash_micros = CAPITAL;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(BPS).unwrap();
    // The Batch 03 baseline execution model: 5 bps slippage, no volatility term.
    c.stress = StressProfile {
        slippage_bps: SLIPPAGE_BPS,
        volatility_mult_bps: 0,
        participation_impact_bps: 0,
    };
    c
}

fn registered() -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(NAME).unwrap()
}

fn run(b: &[BacktestBar]) -> mqk_backtest::BacktestReport {
    let mut e = BacktestEngine::new(cfg());
    e.add_strategy(registered()).unwrap();
    let report = e.run(b).unwrap();
    assert!(!report.halted, "{:?}", report.halt_reason);
    report
}

/// Independent reference: windows by walking BACKWARDS over scheduled sessions from every
/// scheduled holiday (the engine looks forward from the session instead).
fn window_sessions() -> BTreeSet<NaiveDate> {
    let mut out = BTreeSet::new();
    let mut x = d(2016, 1, 11);
    while x <= sessions_v2::coverage_end() {
        if sessions_v2::is_scheduled_holiday(x).unwrap() {
            let mut cursor = x;
            for _ in 0..2 {
                cursor = cursor.pred_opt().unwrap();
                while !sessions_v2::is_scheduled_session(cursor).unwrap() {
                    cursor = cursor.pred_opt().unwrap();
                }
                out.insert(cursor);
            }
        }
        x = x.succ_opt().unwrap();
    }
    out
}

fn reference_targets(dates: &[NaiveDate]) -> Vec<i64> {
    let window = window_sessions();
    dates
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == 0 {
                return 0; // one bar only: the declared two-bar history does not exist
            }
            let n = sessions_v2::next_scheduled_session_after(*t).unwrap();
            i64::from(window.contains(&n)) * M
        })
        .collect()
}

fn buy_price(high: i64) -> i64 {
    high + high * SLIPPAGE_BPS / 10_000
}

fn sell_price(low: i64) -> i64 {
    low - low * SLIPPAGE_BPS / 10_000
}

fn index_of(dates: &[NaiveDate], day: NaiveDate) -> usize {
    dates.iter().position(|x| *x == day).unwrap()
}

#[test]
fn emitted_stream_equals_the_independent_reference_at_every_bar() {
    let dates = session_dates();
    let s = series(dates.len());
    let b = bars(&dates, &s);
    assert_eq!(registered().required_history_bars(), 2);
    let stream = emit_native_signal_stream(cfg(), &b, registered()).unwrap();
    assert_eq!(stream.rows.len(), dates.len());
    // Capital-fraction streams carry the sized absolute target; compare the long/flat decision.
    let got: Vec<bool> = stream
        .rows
        .iter()
        .map(|r| r.target_qty_micros > 0)
        .collect();
    let expect: Vec<bool> = reference_targets(&dates).iter().map(|q| *q > 0).collect();
    assert_eq!(got, expect, "emitter vs independent reference");
    assert!(got.iter().filter(|x| **x).count() > 20 && got.iter().filter(|x| !**x).count() > 200);
    // The emitter records the strategy the engine actually executes: the capital-fraction wrapper.
    let report = run(&b);
    assert_eq!(
        stream.semantic_fingerprint,
        report.strategy_semantic_fingerprint
    );
    assert_eq!(stream.run_id, report.run_id);
    assert_eq!(stream.sizing_provenance, report.sizing_provenance);
    assert_eq!(stream.required_history_bars, 2);
    // The inner engine's own fingerprint is what a fixed-quantity emission records.
    let mut fixed = BacktestConfig::conservative_defaults();
    fixed.timeframe_secs = DAY;
    fixed.integrity_enabled = false;
    let inner = emit_native_signal_stream(fixed, &b, registered()).unwrap();
    assert_eq!(
        inner.semantic_fingerprint,
        registered().semantic_fingerprint()
    );
    assert_ne!(inner.semantic_fingerprint, stream.semantic_fingerprint);
}

#[test]
fn thanksgiving_2024_entry_hold_exit_and_fill_timing_are_causal_and_exact() {
    let dates = session_dates();
    let s = series(dates.len());
    let report = run(&bars(&dates, &s));
    let (p3, p2, p1, reopen) = (
        d(2024, 11, 25),
        d(2024, 11, 26),
        d(2024, 11, 27),
        d(2024, 11, 29),
    );
    let buy = report
        .fills
        .iter()
        .find(|f| f.signal_ts == label(p3))
        .expect("entry order decided on the completed P3 bar");
    // Entry: decided on P3, filled on the NEXT bar (the first window session), never the signal bar.
    assert_eq!(buy.fill_ts, label(p2));
    assert!(buy.fill_ts > buy.signal_ts);
    assert_eq!(buy.inner.side, Side::Buy);
    let i3 = index_of(&dates, p3);
    let i2 = index_of(&dates, p2);
    let q = (CAPITAL * BPS / 10_000 / s.c[i3]) * M;
    assert_eq!(
        buy.inner.qty.raw(),
        q,
        "Q = floor(USD 10,000 / close of the signal bar)"
    );
    assert_eq!(
        buy.inner.price_micros,
        buy_price(s.h[i2]),
        "priced from the FILL bar's high + 5 bps"
    );
    // Hold: nothing is traded on the P2 decision or the P1 entry... only the P1 decision exits.
    assert!(
        !report.fills.iter().any(|f| f.signal_ts == label(p2)),
        "no re-entry or resize while held"
    );
    let sell = report
        .fills
        .iter()
        .find(|f| f.signal_ts == label(p1))
        .expect("exit order decided on the completed P1 bar");
    assert_eq!(
        sell.fill_ts,
        label(reopen),
        "exit fills on the reopening session after the holiday"
    );
    assert_eq!(sell.inner.side, Side::Sell);
    assert_eq!(
        sell.inner.qty.raw(),
        q,
        "the exit closes exactly the entry quantity"
    );
    assert_eq!(
        sell.inner.price_micros,
        sell_price(s.l[index_of(&dates, reopen)])
    );
    // The holiday gap itself (Thanksgiving) is inside the holding period.
    assert!(label(d(2024, 11, 28)) > buy.fill_ts && label(d(2024, 11, 28)) < sell.fill_ts);
}

#[test]
fn every_round_trip_matches_the_independent_windows_and_never_fills_on_the_signal_bar() {
    let dates = session_dates();
    let s = series(dates.len());
    let report = run(&bars(&dates, &s));
    let window = window_sessions();
    let target = reference_targets(&dates);
    assert!(!report.fills.is_empty());
    for f in &report.fills {
        assert!(f.fill_ts > f.signal_ts, "no same-bar fill");
    }
    // Expected order decisions: a buy where the target rises 0 -> long, a sell where it falls.
    let mut expected: Vec<(i64, Side)> = Vec::new();
    for i in 1..dates.len() {
        let (prev, now) = (target[i - 1] > 0, target[i] > 0);
        if now && !prev {
            expected.push((label(dates[i]), Side::Buy));
        }
        if prev && !now {
            expected.push((label(dates[i]), Side::Sell));
        }
    }
    let got: Vec<(i64, Side)> = report
        .fills
        .iter()
        .map(|f| (f.signal_ts, f.inner.side))
        .collect();
    // Orders whose fill bar would lie beyond the series are not produced as fills; the fixture
    // ends flat, so the sets are equal.
    assert_eq!(got, expected);
    // Alternating buy/sell, every buy's signal session is P3 (the session before a window start).
    for (k, f) in report.fills.iter().enumerate() {
        assert_eq!(
            f.inner.side,
            if k % 2 == 0 { Side::Buy } else { Side::Sell }
        );
        let signal_date = dates[dates.iter().position(|x| label(*x) == f.signal_ts).unwrap()];
        let n = sessions_v2::next_scheduled_session_after(signal_date).unwrap();
        assert_eq!(
            window.contains(&n),
            f.inner.side == Side::Buy,
            "{signal_date}"
        );
    }
    assert_eq!(report.fills.len() % 2, 0, "flat at the end");
    // Quantities, entry sizing provenance and fills agree.
    assert_eq!(
        report.sizing_provenance.entries.len(),
        report.fills.len() / 2
    );
    for (entry, buy) in report
        .sizing_provenance
        .entries
        .iter()
        .zip(report.fills.iter().step_by(2))
    {
        assert_eq!(entry.resolved_target_qty_micros, buy.inner.qty.raw());
        assert_eq!(entry.position_budget_micros, 10_000 * M);
        assert_ne!(
            entry.resolved_target_qty_micros, M,
            "never the raw +1 share"
        );
    }
}

#[test]
fn costs_and_slippage_reconcile_exactly_to_final_equity() {
    let dates = session_dates();
    let s = series(dates.len());
    let b = bars(&dates, &s);
    let report = run(&b);
    let mut pnl: i128 = 0;
    let mut fees: i128 = 0;
    for f in &report.fills {
        let notional =
            i128::from(f.inner.price_micros) * i128::from(f.inner.qty.raw()) / i128::from(M);
        match f.inner.side {
            Side::Buy => pnl -= notional,
            Side::Sell => pnl += notional,
        }
        let shares = i128::from(f.inner.qty.raw() / M);
        assert_eq!(
            i128::from(f.inner.fee_micros),
            shares * i128::from(COMMISSION_PER_SHARE_MICROS)
        );
        fees += i128::from(f.inner.fee_micros);
        // Slippage is applied against the trader from the fill bar's own range.
        let bar = &b[dates.iter().position(|x| label(*x) == f.fill_ts).unwrap()];
        match f.inner.side {
            Side::Buy => assert_eq!(f.inner.price_micros, buy_price(bar.high_micros)),
            Side::Sell => assert_eq!(f.inner.price_micros, sell_price(bar.low_micros)),
        }
    }
    let final_equity = i128::from(report.equity_curve.last().unwrap().1);
    assert_eq!(final_equity, i128::from(CAPITAL) + pnl - fees);
    assert!(fees > 0);
}

#[test]
fn a_missing_window_bar_never_creates_a_phantom_or_extends_a_position() {
    let dates = session_dates();
    let s = series(dates.len());
    let full = bars(&dates, &s);
    let (p3, p2, p1, reopen) = (
        d(2024, 11, 25),
        d(2024, 11, 26),
        d(2024, 11, 27),
        d(2024, 11, 29),
    );

    // (a) The decision bar P3 is missing: P2 sees a non-consecutive pair, so no entry exists.
    let without_p3: Vec<BacktestBar> = full
        .iter()
        .filter(|b| b.end_ts != label(p3))
        .cloned()
        .collect();
    let r = run(&without_p3);
    assert!(
        !r.fills
            .iter()
            .any(|f| f.fill_ts >= label(d(2024, 11, 22)) && f.fill_ts <= label(reopen)),
        "no trade at all for the Thanksgiving window when its decision bar is missing"
    );

    // (b) P2 is missing after a valid entry decision on P3: the order fills on the next available
    // bar (P1), and the P1 decision sees a gap and goes flat; exit on the reopening session.
    let without_p2: Vec<BacktestBar> = full
        .iter()
        .filter(|b| b.end_ts != label(p2))
        .cloned()
        .collect();
    let r = run(&without_p2);
    let buy = r.fills.iter().find(|f| f.signal_ts == label(p3)).unwrap();
    assert_eq!(
        buy.fill_ts,
        label(p1),
        "fills on the next AVAILABLE bar, causally"
    );
    let sell = r.fills.iter().find(|f| f.signal_ts == label(p1)).unwrap();
    assert_eq!(sell.inner.side, Side::Sell);
    assert_eq!(sell.fill_ts, label(reopen));

    // (c) A missing reopening bar delays the exit fill to the next available bar, never earlier.
    let without_reopen: Vec<BacktestBar> = full
        .iter()
        .filter(|b| b.end_ts != label(reopen))
        .cloned()
        .collect();
    let r = run(&without_reopen);
    let sell = r.fills.iter().find(|f| f.signal_ts == label(p1)).unwrap();
    assert_eq!(sell.fill_ts, label(d(2024, 12, 2)));
}

#[test]
fn a_surprise_closure_inside_the_series_changes_no_decision() {
    // 2025-01-09 has no bar (surprise closure). The series around it, and every fill, is
    // identical to the independent reference computed from the scheduled calendar alone.
    let dates = session_dates();
    assert!(!dates.contains(&d(2025, 1, 9)));
    let s = series(dates.len());
    let b = bars(&dates, &s);
    let stream = emit_native_signal_stream(cfg(), &b, registered()).unwrap();
    let around = |day: NaiveDate| stream.rows[index_of(&dates, day)].target_qty_micros;
    for day in [d(2025, 1, 7), d(2025, 1, 8), d(2025, 1, 10), d(2025, 1, 13)] {
        assert_eq!(around(day), 0, "{day}");
    }
    // The next real window (MLK Day 2025-01-20: P2 = Thu 1-16, P1 = Fri 1-17) is unaffected.
    assert!(around(d(2025, 1, 15)) > 0 && around(d(2025, 1, 16)) > 0);
    assert_eq!(around(d(2025, 1, 17)), 0);
}

#[test]
fn a_fresh_instance_with_only_the_bounded_history_reproduces_the_long_lived_stream() {
    let dates = session_dates();
    let s = series(dates.len());
    let mut long_lived = registered();
    for t in 0..dates.len() {
        let lo = t.saturating_sub(300);
        let full: Vec<BarStub> = (lo..=t)
            .map(|i| BarStub::new(label(dates[i]), true, s.c[i], 1))
            .collect();
        let latest: Vec<BarStub> = full[full.len().saturating_sub(2)..].to_vec();
        let ctx_of = |w: Vec<BarStub>| {
            StrategyContext::new(DAY, 0, RecentBarsWindow::new(w.len().max(1), w))
        };
        let a = long_lived.on_bar(&ctx_of(full)).targets[0].qty.raw();
        let b = registered().on_bar(&ctx_of(latest)).targets[0].qty.raw();
        assert_eq!(a, b, "bar {t}");
    }
}

#[test]
fn registry_metadata_and_fingerprint_binding() {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let meta = reg
        .list()
        .into_iter()
        .find(|m| m.name == NAME)
        .unwrap()
        .clone();
    assert_eq!(meta.timeframe_secs, DAY);
    assert_eq!(meta.data_requirements.unwrap().minimum_completed_bars, 2);
    assert_eq!(
        meta.restart_recovery,
        RestartRecovery::BoundedHistoryReconstructible
    );
    let fp = |sym: &str| {
        let mut r = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut r, sym, 1, None, None).unwrap();
        r.instantiate(NAME).unwrap().semantic_fingerprint()
    };
    let all: BTreeSet<String> = ["SPY", "QQQ", "IWM", "DIA"].iter().map(|s| fp(s)).collect();
    assert_eq!(all.len(), 4, "one distinct fingerprint per symbol");
    let dates = session_dates();
    let b = bars(&dates, &series(dates.len()));
    let report = run(&b);
    assert_eq!(report.strategy_semantic_fingerprint.len(), 64);
    assert_ne!(
        report.strategy_semantic_fingerprint,
        fp("SPY"),
        "the wrapper fingerprint binds the sizing contract"
    );
}

fn held_record(entry_close_micros: i64, entry_ts: i64) -> HeldSizingRecord {
    HeldSizingRecord {
        deployment_id: "dep-1".into(),
        strategy_id: NAME.into(),
        symbol: "SPY".into(),
        state_version: HELD_SIZING_STATE_VERSION,
        entry_generation: 1,
        status: HeldSizingStatus::Active,
        policy_id: SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1.into(),
        allocation_fraction_bps: BPS,
        initial_allocated_capital_micros: CAPITAL,
        max_target_qty_micros: None,
        max_notional_usd: None,
        resolved_target_qty_micros: (CAPITAL * BPS / 10_000 / entry_close_micros) * M,
        reference_bar_end_ts: entry_ts,
        reference_price_micros: entry_close_micros,
    }
}

fn recoverable(
    snapshot: Vec<HeldSizingRecord>,
) -> (
    CapitalFractionSizedStrategy,
    mqk_strategy::SizingStateHandle,
) {
    let (w, _audit, state) = CapitalFractionSizedStrategy::new_recoverable(
        registered_inner(),
        SizingPolicy::capital_fraction_v1(BPS).unwrap(),
        CAPITAL,
        TargetSizing::equity_default(),
        HeldSizingScope {
            deployment_id: "dep-1".into(),
            strategy_id: NAME.into(),
        },
        snapshot,
    )
    .unwrap();
    (w, state)
}

fn registered_inner() -> Box<dyn Strategy> {
    Box::new(mqk_strategy::engines::PreHolidayTwoSessionLongV1Strategy::new("SPY"))
}

fn ctx(prev: NaiveDate, prev_close: i64, latest: NaiveDate, latest_close: i64) -> StrategyContext {
    StrategyContext::new(
        DAY,
        0,
        RecentBarsWindow::new(
            2,
            vec![
                BarStub::new(label(prev), true, prev_close, 1),
                BarStub::new(label(latest), true, latest_close, 1),
            ],
        ),
    )
}

#[test]
fn restart_mid_window_with_the_durable_record_holds_the_exact_quantity_and_exits_on_schedule() {
    let (p3, p2, p1) = (d(2024, 11, 25), d(2024, 11, 26), d(2024, 11, 27));
    let entry_close = 452_340_000; // USD 452.34 -> floor(10,000 / 452.34) = 22 shares
    let (mut w, state) = recoverable(vec![held_record(entry_close, label(p3))]);
    // First decision after the restart is the P2 completion (N = P1, in the window): a much
    // higher close must NOT re-resolve the quantity.
    let out = w.on_bar(&ctx(p3, entry_close, p2, 900_000_000));
    assert_eq!(
        out.targets[0].qty.raw(),
        22 * M,
        "exactly the original Q, no one-share fallback"
    );
    assert!(
        state.drain_transitions().is_empty(),
        "no duplicate entry after the restart"
    );
    // The P1 completion is the exit signal: target flat, the record is released exactly once.
    let out = w.on_bar(&ctx(p2, 900_000_000, p1, 1_000_000));
    assert_eq!(out.targets[0].qty.raw(), 0);
    let transitions = state.drain_transitions();
    assert_eq!(transitions.len(), 1, "exactly one release transition");
    assert!(matches!(
        transitions[0],
        mqk_strategy::HeldSizingTransition::Released(_)
    ));
    // Restarting exactly on the exit decision with the record still Active also releases it once.
    let (mut w, state) = recoverable(vec![held_record(entry_close, label(p3))]);
    assert_eq!(
        w.on_bar(&ctx(p2, 900_000_000, p1, 1_000_000)).targets[0]
            .qty
            .raw(),
        0
    );
    assert_eq!(state.drain_transitions().len(), 1);
}

#[test]
fn restart_with_no_record_is_flat_state_never_a_phantom_position() {
    let (p3, p2, p1) = (d(2024, 11, 25), d(2024, 11, 26), d(2024, 11, 27));
    // No durable record: nothing is held. Outside any window the wrapper emits flat.
    let (mut w, state) = recoverable(vec![]);
    let out = w.on_bar(&ctx(
        d(2024, 11, 21),
        450_000_000,
        d(2024, 11, 22),
        451_000_000,
    ));
    assert_eq!(out.targets[0].qty.raw(), 0);
    assert!(state.drain_transitions().is_empty());
    // Mid-window with no record the engine says long: this is a FRESH entry sized from the
    // current completed close (recorded as a new generation), not a recovered position.
    let (mut w, state) = recoverable(vec![]);
    let out = w.on_bar(&ctx(p3, 450_000_000, p2, 500_000_000));
    assert_eq!(
        out.targets[0].qty.raw(),
        20 * M,
        "floor(10,000 / 500.00) from the current close"
    );
    assert_eq!(state.drain_transitions().len(), 1);
    // The exit decision releases it; the engine never invents a held position on its own.
    let out = w.on_bar(&ctx(p2, 500_000_000, p1, 501_000_000));
    assert_eq!(out.targets[0].qty.raw(), 0);
    assert_eq!(state.drain_transitions().len(), 1);
}
