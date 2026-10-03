//! Research bridge for FixedInitialCapitalFractionV1: the native signal stream
//! carries the SAME absolute sized targets, capital basis and wrapper identity
//! the canonical Backtest executes. Synthetic fixtures only.

use mqk_backtest::{
    capital_fraction_wrapped_fingerprint, emit_native_signal_stream, BacktestBar, BacktestConfig,
    BacktestEngine, BacktestReport, SizingPolicy, StrategySizingConfig,
};
use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy, StrategyContext, StrategySpec};

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;

struct Scripted {
    script: Vec<bool>,
    idx: usize,
}

impl Scripted {
    fn boxed(script: Vec<bool>) -> Box<dyn Strategy> {
        Box::new(Self { script, idx: 0 })
    }
}

impl Strategy for Scripted {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("scripted", DAY)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let long = self.script.get(self.idx).copied().unwrap_or(false);
        self.idx += 1;
        let qty = if long {
            QtyMicros::new(M)
        } else {
            QtyMicros::ZERO
        };
        StrategyOutput::new(vec![TargetPosition::new("SPY", qty)])
    }
}

fn bar(i: i64, open: i64, close: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        DAY * (i + 1),
        open * M,
        open.max(close) * M,
        open.min(close) * M,
        close * M,
        10_000_000,
    )
}

fn config(bps: Option<i64>, capital: i64) -> BacktestConfig {
    let mut c = BacktestConfig::test_defaults();
    c.timeframe_secs = DAY;
    c.initial_cash_micros = capital;
    c.max_gross_exposure_mult_micros = 100_000_000;
    c.integrity_enabled = false;
    if let Some(b) = bps {
        c.sizing_policy = SizingPolicy::capital_fraction_v1(b).unwrap();
    }
    c
}

/// entry @100 -> exit -> re-entry @125.
fn bars_reentry() -> Vec<BacktestBar> {
    vec![
        bar(0, 100, 100),
        bar(1, 100, 100), // entry decision, close 100
        bar(2, 111, 200), // fill price is NOT the sizing price
        bar(3, 200, 250),
        bar(4, 250, 125), // inner flat decision
        bar(5, 125, 125),
        bar(6, 125, 125), // re-entry decision, close 125
        bar(7, 125, 125),
    ]
}

fn script_reentry() -> Vec<bool> {
    vec![false, true, true, true, false, false, true, true]
}

fn canonical(cfg: BacktestConfig, script: Vec<bool>, bars: &[BacktestBar]) -> BacktestReport {
    let mut e = BacktestEngine::new(cfg);
    e.add_strategy(Scripted::boxed(script)).unwrap();
    e.run(bars).unwrap()
}

#[test]
fn emitted_targets_equal_the_sized_backtest_targets_not_plus_one() {
    let cfg = config(Some(2_500), CAPITAL);
    let stream = emit_native_signal_stream(
        cfg.clone(),
        &bars_reentry(),
        Scripted::boxed(script_reentry()),
    )
    .unwrap();
    let report = canonical(cfg, script_reentry(), &bars_reentry());

    let q: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
    // 25% of 100k: 250 shares at close 100, then 200 at close 125 (initial
    // capital, new causal close); never the inner +1.
    assert_eq!(
        q,
        vec![0, 250 * M, 250 * M, 250 * M, 0, 0, 200 * M, 200 * M]
    );
    assert!(stream.rows.iter().all(|r| r.target_qty_micros != M));
    assert_eq!(stream.sizing_provenance, report.sizing_provenance);
    assert_eq!(stream.sizing_provenance.entries.len(), 2);
    assert_eq!(
        report
            .fills
            .iter()
            .map(|f| f.inner.qty.raw())
            .filter(|&x| x > 0)
            .collect::<Vec<_>>()
            .first()
            .copied(),
        Some(250 * M)
    );
}

#[test]
fn stream_identity_equals_canonical_backtest_and_pure_wrapper_fingerprint() {
    let cfg = config(Some(2_500), CAPITAL);
    let stream = emit_native_signal_stream(
        cfg.clone(),
        &bars_reentry(),
        Scripted::boxed(script_reentry()),
    )
    .unwrap();
    let report = canonical(cfg.clone(), script_reentry(), &bars_reentry());
    assert_eq!(
        stream.semantic_fingerprint,
        report.strategy_semantic_fingerprint
    );
    assert_eq!(stream.run_id, report.run_id, "same engine run identity");
    let inner = Scripted::boxed(vec![]).semantic_fingerprint();
    assert_eq!(
        capital_fraction_wrapped_fingerprint(&cfg, &inner).unwrap(),
        stream.semantic_fingerprint
    );
    assert_ne!(stream.semantic_fingerprint, inner);
    assert_eq!(stream.initial_cash_micros, CAPITAL);
}

#[test]
fn wrapped_fingerprint_binds_fraction_capital_and_caps_without_market_data() {
    let inner = Scripted::boxed(vec![]).semantic_fingerprint();
    let fp = |c: &BacktestConfig| capital_fraction_wrapped_fingerprint(c, &inner).unwrap();
    let base = fp(&config(Some(1_000), CAPITAL));
    assert_ne!(base, fp(&config(Some(2_000), CAPITAL)), "1000 -> 2000 bps");
    assert_ne!(base, fp(&config(Some(1_000), 200_000 * M)), "capital");
    let mut capped = config(Some(1_000), CAPITAL);
    capped.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: Some(10),
        max_position_notional_usd: None,
    };
    assert_ne!(base, fp(&capped), "caps");
    assert_eq!(base, fp(&config(Some(1_000), CAPITAL)), "deterministic");
    // The fixed policy has no wrapper identity and no default fraction.
    assert!(capital_fraction_wrapped_fingerprint(&config(None, CAPITAL), &inner).is_err());
}

#[test]
fn future_bars_cannot_change_already_resolved_signal_quantity() {
    let cfg = config(Some(2_500), CAPITAL);
    let a = emit_native_signal_stream(
        cfg.clone(),
        &bars_reentry(),
        Scripted::boxed(script_reentry()),
    )
    .unwrap();
    let mut altered = bars_reentry();
    for (i, b) in altered.iter_mut().enumerate().skip(3) {
        *b = bar(i as i64, 1, 1);
    }
    let b = emit_native_signal_stream(cfg, &altered, Scripted::boxed(script_reentry())).unwrap();
    assert_eq!(a.rows[..3], b.rows[..3]);
    assert_eq!(a.rows[1].target_qty_micros, 250 * M);
}

#[test]
fn insufficient_budget_emits_flat_never_one_share() {
    let cfg = config(Some(1), CAPITAL); // 10 USD budget < price
    let stream =
        emit_native_signal_stream(cfg, &bars_reentry(), Scripted::boxed(vec![true; 8])).unwrap();
    assert!(stream.rows.iter().all(|r| r.target_qty_micros == 0));
    assert!(!stream.sizing_provenance.refusals.is_empty());
}

#[test]
fn fixed_quantity_stream_is_unchanged() {
    let cfg = config(None, CAPITAL);
    let stream = emit_native_signal_stream(
        cfg.clone(),
        &bars_reentry(),
        Scripted::boxed(script_reentry()),
    )
    .unwrap();
    let report = canonical(cfg, script_reentry(), &bars_reentry());
    assert_eq!(stream.sizing_policy, SizingPolicy::FixedQuantityV1);
    assert_eq!(stream.sizing_provenance, Default::default());
    assert_eq!(
        stream.semantic_fingerprint,
        Scripted::boxed(vec![]).semantic_fingerprint(),
        "fixed-quantity identity is the unwrapped strategy fingerprint"
    );
    assert_eq!(stream.run_id, report.run_id);
    assert!(stream.rows.iter().all(|r| r.target_qty_micros <= M));
}

#[test]
fn native_registry_strategy_emits_capital_sized_targets_matching_backtest() {
    const N: usize = 320;
    let bars: Vec<BacktestBar> = (0..N as i64)
        .map(|i| {
            let c = 100_000_000 + i * 50_000;
            BacktestBar::new("SPY", DAY * (i + 1), c, c + M, c - M, c, 1_000)
        })
        .collect();
    let instantiate = || {
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
        reg.instantiate("trend_sma50").unwrap()
    };
    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = DAY;
    cfg.integrity_enabled = false;
    cfg.initial_cash_micros = CAPITAL;
    cfg.sizing_policy = SizingPolicy::capital_fraction_v1(1_000).unwrap();

    let stream = emit_native_signal_stream(cfg.clone(), &bars, instantiate()).unwrap();
    let mut e = BacktestEngine::new(cfg);
    e.add_strategy(instantiate()).unwrap();
    let report = e.run(&bars).unwrap();

    let entry = &report.sizing_provenance.entries[0];
    assert!(
        entry.resolved_target_qty_micros > M,
        "10% of 100k exceeds 1 share"
    );
    let emitted: Vec<i64> = stream
        .rows
        .iter()
        .map(|r| r.target_qty_micros)
        .filter(|&q| q > 0)
        .collect();
    assert!(!emitted.is_empty());
    assert_eq!(emitted[0], entry.resolved_target_qty_micros);
    assert_eq!(
        stream.semantic_fingerprint,
        report.strategy_semantic_fingerprint
    );
    assert_eq!(stream.run_id, report.run_id);
}
