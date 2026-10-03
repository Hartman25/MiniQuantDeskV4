//! Benchmark run identity (IR-SZ-03): the capital-fraction benchmark's canonical
//! `run_id` binds its exact target quantity and causal entry, and the evidence
//! verifier recomputes it, so a substituted run id is refused at review and
//! promotion. Fixtures are synthetic.

use mqk_backtest::benchmark_capital_fraction::{
    capital_fraction_benchmark_semantic_fingerprint, compute_capital_fraction_benchmark,
    expected_capital_fraction_benchmark_run_id, CapitalFractionBenchmarkSection,
};
use mqk_backtest::{
    evaluate_scan_candidate, BacktestBar, BacktestConfig, BacktestEngine, BacktestReport,
    ScanBenchmarkPolicy, ScanCapitalFractionBenchmarkEvidence, SizingPolicy, StrategyScanPolicy,
};
use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy, StrategyContext, StrategySpec};
use uuid::Uuid;

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;

/// Long from decision index `from` onward.
struct LongFrom {
    from: usize,
    seen: usize,
}

impl Strategy for LongFrom {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("long_from", DAY)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let i = self.seen;
        self.seen += 1;
        StrategyOutput::new(vec![TargetPosition::new(
            "SPY",
            if i >= self.from {
                QtyMicros::from_whole_units(1).unwrap()
            } else {
                QtyMicros::ZERO
            },
        )])
    }
}

fn flat_bars(close: i64, n: i64) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            BacktestBar::new(
                "SPY",
                DAY * (i + 1),
                close * M,
                close * M + M,
                close * M - M,
                close * M,
                1_000,
            )
        })
        .collect()
}

fn cfg(bps: i64) -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c.initial_cash_micros = CAPITAL;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(bps).unwrap();
    c
}

fn candidate(cfg: &BacktestConfig, bars: &[BacktestBar], from: usize) -> BacktestReport {
    let mut e = BacktestEngine::new(cfg.clone());
    e.add_strategy(Box::new(LongFrom { from, seen: 0 }))
        .unwrap();
    e.run(bars).unwrap()
}

fn bench(
    cfg: &BacktestConfig,
    bars: &[BacktestBar],
    from: usize,
) -> CapitalFractionBenchmarkSection {
    compute_capital_fraction_benchmark(&candidate(cfg, bars, from), bars, cfg, DAY, 0.0).unwrap()
}

#[test]
fn same_bars_and_cost_config_but_different_q_have_different_benchmark_run_ids() {
    let bars = flat_bars(100, 12);
    let (a, b) = (bench(&cfg(1_000), &bars, 2), bench(&cfg(2_000), &bars, 2));
    // Premise: the benchmark config is the candidate config with only the sizing
    // policy swapped, so the two benchmarks share one config id and one data hash.
    assert_eq!(a.config_id, b.config_id);
    assert_ne!(
        a.target_qty_micros, b.target_qty_micros,
        "Q differs: 100 vs 200 shares"
    );
    assert_ne!(
        a.benchmark_run_id, b.benchmark_run_id,
        "an economically different passive benchmark must not share a run id"
    );
}

#[test]
fn same_q_but_a_different_causal_entry_have_different_benchmark_run_ids() {
    let bars = flat_bars(100, 12);
    let c = cfg(1_000);
    let (a, b) = (bench(&c, &bars, 2), bench(&c, &bars, 5));
    assert_eq!(
        a.target_qty_micros, b.target_qty_micros,
        "same Q (flat price)"
    );
    assert_eq!(a.config_id, b.config_id);
    assert_ne!(a.entry_bar_index, b.entry_bar_index);
    assert_ne!(a.benchmark_run_id, b.benchmark_run_id);
}

#[test]
fn identical_benchmark_behavior_is_replay_stable_and_matches_the_pure_derivation() {
    let bars = flat_bars(100, 12);
    let c = cfg(1_000);
    let (a, b) = (bench(&c, &bars, 2), bench(&c, &bars, 2));
    assert_eq!(a.benchmark_run_id, b.benchmark_run_id);
    let candidate_report = candidate(&c, &bars, 2);
    let expected = expected_capital_fraction_benchmark_run_id(
        &Uuid::parse_str(&a.config_id).unwrap(),
        &candidate_report.input_data_hash,
        &a.execution_model_id,
        &a.symbol,
        DAY,
        a.target_qty_micros,
        a.reference_bar_end_ts,
        a.entry_bar_index,
    );
    assert_eq!(a.benchmark_run_id, expected.to_string());
}

#[test]
fn every_behavior_bearing_input_changes_the_benchmark_fingerprint() {
    let base = capital_fraction_benchmark_semantic_fingerprint("SPY", DAY, 100 * M, 3 * DAY, 2);
    assert_eq!(
        base,
        capital_fraction_benchmark_semantic_fingerprint("SPY", DAY, 100 * M, 3 * DAY, 2)
    );
    for (name, other) in [
        (
            "symbol",
            capital_fraction_benchmark_semantic_fingerprint("QQQ", DAY, 100 * M, 3 * DAY, 2),
        ),
        (
            "timeframe",
            capital_fraction_benchmark_semantic_fingerprint("SPY", 3_600, 100 * M, 3 * DAY, 2),
        ),
        (
            "target Q",
            capital_fraction_benchmark_semantic_fingerprint("SPY", DAY, 101 * M, 3 * DAY, 2),
        ),
        (
            "reference bar ts",
            capital_fraction_benchmark_semantic_fingerprint("SPY", DAY, 100 * M, 4 * DAY, 2),
        ),
        (
            "entry index",
            capital_fraction_benchmark_semantic_fingerprint("SPY", DAY, 100 * M, 3 * DAY, 3),
        ),
    ] {
        assert_ne!(base, other, "{name}");
    }
}

fn rising(n: i64) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            let c = 100_000_000 + i * 50_000;
            BacktestBar::new("SPY", DAY * (i + 1), c, c + M, c - M, c, 1_000)
        })
        .collect()
}

fn genuine_evidence() -> ScanCapitalFractionBenchmarkEvidence {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(2_500).unwrap();
    let policy = StrategyScanPolicy {
        base_config: c,
        benchmark_policy: ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
        ..StrategyScanPolicy::default()
    };
    let cand = evaluate_scan_candidate(
        "SPY",
        "1D",
        "absolute_momentum_252",
        Some(DAY),
        Some(reg.instantiate("absolute_momentum_252").unwrap()),
        Some(&rising(320)),
        &policy,
    );
    cand.metrics
        .benchmark_capital_fraction
        .expect("genuine capital-fraction evidence")
}

#[test]
fn the_verifier_recomputes_the_run_id_and_refuses_substitution_or_tampered_inputs() {
    let good = genuine_evidence();
    good.verify_internal()
        .expect("control: genuine evidence verifies");

    let mut substituted = good.clone();
    substituted.benchmark_run_id =
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"another benchmark run").to_string();
    let err = substituted.verify_internal().unwrap_err();
    assert!(err.contains("benchmark_run_id does not match"), "{err}");

    type Tamper = fn(&mut ScanCapitalFractionBenchmarkEvidence);
    let tampers: Vec<(&str, Tamper)> = vec![
        ("benchmark entry index", |e| {
            e.benchmark_entry_bar_index += 1
        }),
        ("reference bar ts", |e| e.reference_bar_end_ts += DAY),
        ("benchmark data hash", |e| {
            e.input_data_hash = "0".repeat(64)
        }),
        ("benchmark config id", |e| {
            e.benchmark_config_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"cfg").to_string()
        }),
        ("timeframe label", |e| e.timeframe = "1H".to_string()),
        ("unresolvable timeframe", |e| e.timeframe = "7D".to_string()),
        ("symbol", |e| e.symbol = "QQQ".to_string()),
    ];
    for (name, tamper) in tampers {
        let mut e = good.clone();
        tamper(&mut e);
        assert!(e.verify_internal().is_err(), "{name} must be refused");
    }
    // A quantity change that keeps every other consistency rule true (candidate
    // and benchmark Q moved together, within budget) is still caught by the run id.
    let mut q = good.clone();
    let scale = 1_000_000;
    let one_less = good.benchmark_target_qty_micros - scale;
    assert!(one_less > 0);
    q.benchmark_target_qty_micros = one_less;
    q.candidate_target_qty_micros = one_less;
    let err = q.verify_internal().unwrap_err();
    assert!(err.contains("benchmark_run_id does not match"), "{err}");
}

/// Historical Benchmark V2 identity must not move (it is a frozen fixed-quantity
/// authority; only the capital-fraction benchmark's identity changed).
#[test]
fn benchmark_v2_benchmark_run_id_is_unchanged() {
    use mqk_backtest::benchmark_v2::compute_benchmark_v2;
    use mqk_backtest::emit_native_signal_stream;
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    let bars = rising(320);
    let stream = emit_native_signal_stream(
        c.clone(),
        &bars,
        reg.instantiate("absolute_momentum_252").unwrap(),
    )
    .unwrap();
    let v2 = compute_benchmark_v2(&stream, &bars, c, 0.0).unwrap();
    assert_eq!(v2.benchmark_run_id, PINNED_V2_BENCHMARK_RUN_ID);
}

/// Value produced by the unchanged Benchmark V2 derivation (strategy name, config, data,
/// execution model and spec-only fingerprint); it must never move.
const PINNED_V2_BENCHMARK_RUN_ID: &str = "0c85a921-4635-5ac4-8575-8190f07d6439";
