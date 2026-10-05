//! The three batch-01 native engines, driven through the real BacktestEngine
//! window and the native signal emitter, must equal an independent integer
//! reference at every bar, declare their true history requirement, and bind the
//! emitter fingerprint to the registered engine.

use mqk_backtest::{emit_native_signal_stream, BacktestBar, BacktestConfig};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, Strategy, StrategyContext, StrategySpec,
};

const DAY: i64 = 86_400;
const N: usize = 900;

fn cfg() -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c
}

/// Rising trend with a repeating five-day dip (events for the pullback engine),
/// a slow multi-year swing (crossings of the 252-bar reference and of the
/// 3% band) and deterministic noise.
fn closes() -> Vec<i64> {
    (0..N)
        .map(|i| {
            let swing = ((i as f64 / 110.0).sin() * 18_000_000.0) as i64;
            let phase = (i % 17) as i64;
            let dip = if (3..=6).contains(&phase) {
                (phase - 2) * 2_500_000
            } else {
                0
            };
            let noise = (((i * 37) % 23) as i64 - 11) * 120_000;
            140_000_000 + i as i64 * 60_000 + swing - dip + noise
        })
        .collect()
}

fn bars(closes: &[i64]) -> Vec<BacktestBar> {
    closes
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            BacktestBar::new(
                "SPY",
                DAY * (i as i64 + 1),
                c,
                c + 500_000,
                c - 500_000,
                c,
                1_000,
            )
        })
        .collect()
}

fn registered(name: &str) -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(name).unwrap()
}

/// Independent reference, integer arithmetic only, evaluated on the full history prefix.
fn reference(name: &str, c: &[i64], t: usize) -> i64 {
    let at = |i: usize| c[i] as i128;
    let sum = |from: usize, to: usize| (from..=to).map(at).sum::<i128>();
    match name {
        "absolute_momentum_252" => {
            if t < 252 {
                return 0;
            }
            i64::from(at(t) > at(t - 252))
        }
        "near_high_momentum_252_3pct" => {
            if t < 251 {
                return 0;
            }
            let high = (t - 251..=t).map(at).max().unwrap();
            i64::from(100 * at(t) >= 97 * high && 50 * at(t) > sum(t - 49, t))
        }
        "trend_pullback_5d_4pct_hold5" => {
            if t < 203 {
                return 0;
            }
            let event = |j: usize| 200 * at(j) > sum(j - 199, j) && 100 * at(j) <= 96 * at(j - 5);
            i64::from((0..5).any(|k| event(t - k)))
        }
        other => panic!("unknown engine {other}"),
    }
}

const ENGINES: [(&str, usize); 3] = [
    ("absolute_momentum_252", 253),
    ("near_high_momentum_252_3pct", 252),
    ("trend_pullback_5d_4pct_hold5", 204),
];

#[test]
fn emitted_stream_equals_the_independent_reference_at_every_bar() {
    let c = closes();
    let b = bars(&c);
    for (name, required) in ENGINES {
        assert_eq!(registered(name).required_history_bars(), required, "{name}");
        let stream = emit_native_signal_stream(cfg(), &b, registered(name)).unwrap();
        assert_eq!(stream.rows.len(), N, "{name}");
        let got: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
        let expect: Vec<i64> = (0..N).map(|t| reference(name, &c, t) * 1_000_000).collect();
        assert_eq!(got, expect, "{name}: emitter vs reference");
        assert!(
            got.iter().any(|&q| q > 0) && got.contains(&0),
            "{name}: fixture must exercise both states"
        );
        assert!(
            got.iter().take(required - 1).all(|&q| q == 0),
            "{name}: flat before the declared history exists"
        );
        assert_eq!(
            stream.semantic_fingerprint,
            registered(name).semantic_fingerprint(),
            "{name}"
        );
    }
}

/// The engines are stateless: a fresh instance fed only its declared bounded
/// history reproduces the long-running instance's target at every bar.
#[test]
fn fresh_instance_with_only_the_bounded_history_matches_at_every_bar() {
    let c = closes();
    for (name, required) in ENGINES {
        let mut long_lived = registered(name);
        for t in 0..N {
            let lo = (t + 1).saturating_sub(required + 40);
            let full: Vec<BarStub> = (lo..=t)
                .map(|i| BarStub::new((i as i64 + 1) * DAY, true, c[i], 1))
                .collect();
            let bounded: Vec<BarStub> = full[full.len().saturating_sub(required)..].to_vec();
            let ctx_of = |w: Vec<BarStub>| {
                StrategyContext::new(DAY, 0, RecentBarsWindow::new(w.len().max(1), w))
            };
            let a = long_lived.on_bar(&ctx_of(full)).targets[0].qty.raw();
            let b = registered(name).on_bar(&ctx_of(bounded)).targets[0]
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

/// Paper loads 275 bars per dispatch (`STRATEGY_CONTEXT_LOAD_LIMIT`, guarded against the whole
/// registered universe in `mqk-daemon`); every batch engine's requirement must fit.
#[test]
fn every_batch_engine_fits_the_paper_context_window() {
    for (name, required) in ENGINES {
        assert!(required <= 275, "{name} needs {required} bars");
    }
}
