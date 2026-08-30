//! STRESS-ROBUSTNESS-SEMANTIC-BINDING-01: `run_backtest_stress_suite` and
//! `run_robustness_gauntlet` accept fresh strategies from `make_strategy()`
//! without proving those instances carry the same semantic fingerprint as
//! the baseline candidate. A caller could therefore produce a baseline
//! candidate A and stress/robustness reruns using semantic strategy B while
//! the output remains labeled with A's baseline identity. Every fresh
//! strategy instance used by a canonical re-run must carry the SAME
//! `strategy_semantic_fingerprint` as the baseline; mismatch must fail
//! closed and never become passing canonical evidence.

use mqk_backtest::{
    run_backtest_stress_suite, run_robustness_gauntlet, BacktestBar, BacktestConfig,
    BacktestEngine, BacktestReport,
};
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use std::cell::Cell;

const MISMATCH_MARKER: &str = "semantic fingerprint mismatch";

/// A strategy whose `spec()` (name/timeframe) is fixed but whose
/// `semantic_fingerprint()` is caller-controlled -- lets tests simulate two
/// materially different strategy semantics (A vs B) sharing an identical
/// name/on_bar behavior, so a fail-closed check can never be explained away
/// as "the results merely differed".
struct FingerprintedStrategy {
    fingerprint: &'static str,
}

impl FingerprintedStrategy {
    fn new(fingerprint: &'static str) -> Self {
        Self { fingerprint }
    }
}

impl Strategy for FingerprintedStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("fp_strategy", 60)
    }

    /// Deliberately hold-flat (never trades): every scenario's own
    /// market-behavior pass criteria (bankruptcy/drawdown/edge-collapse)
    /// trivially clear regardless of fingerprint value, isolating these
    /// tests to the identity-binding invariant itself.
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }

    fn semantic_fingerprint(&self) -> String {
        self.fingerprint.to_string()
    }
}

fn bars() -> Vec<BacktestBar> {
    (0..5)
        .map(|i| {
            let ts = 1_000 + i * 60;
            BacktestBar::new("AAPL", ts, 100, 100, 100, 100, 1_000)
        })
        .collect()
}

/// Runs a real `BacktestEngine` with a `FingerprintedStrategy(fingerprint)`
/// to produce a genuine baseline `BacktestReport` -- never hand-constructed.
fn baseline_report(config: &BacktestConfig, bars: &[BacktestBar], fingerprint: &'static str) -> BacktestReport {
    let mut engine = BacktestEngine::new(config.clone());
    engine
        .add_strategy(Box::new(FingerprintedStrategy::new(fingerprint)))
        .unwrap();
    engine.run(bars).unwrap()
}

fn reason_contains_mismatch(reason: &Option<String>) -> bool {
    reason.as_deref().unwrap_or("").contains(MISMATCH_MARKER)
}

// ---------------------------------------------------------------------------
// 1. baseline A + stress factory A => normal behavior
// ---------------------------------------------------------------------------

#[test]
fn stress_suite_matching_factory_never_reports_identity_mismatch() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_backtest_stress_suite(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-a")));

    assert!(
        output.scenarios.iter().all(|s| !reason_contains_mismatch(&s.reason)),
        "matching factory must never be rejected as an identity mismatch: {:?}",
        output.scenarios
    );
    assert!(
        output.all_passed(),
        "hold-flat strategy with matching identity must clear every stress scenario: {:?}",
        output.scenarios
    );
}

#[test]
fn robustness_gauntlet_matching_factory_never_reports_identity_mismatch() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_robustness_gauntlet(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-a")));

    assert!(
        output.scenarios.iter().all(|s| !reason_contains_mismatch(&s.reason)),
        "matching factory must never be rejected as an identity mismatch: {:?}",
        output.scenarios
    );
}

// ---------------------------------------------------------------------------
// 2. baseline A + stress factory B => fail closed
// ---------------------------------------------------------------------------

#[test]
fn stress_suite_mismatched_factory_fails_closed() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_backtest_stress_suite(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-b")));

    assert!(
        !output.all_passed(),
        "a stress suite built from a semantically mismatched factory must never pass"
    );
    assert!(
        output.scenarios.iter().all(|s| !s.passed),
        "every scenario must fail closed on a mismatched factory: {:?}",
        output.scenarios
    );
    assert!(
        output.scenarios.iter().all(|s| reason_contains_mismatch(&s.reason)),
        "every failure must be explicitly attributed to the identity mismatch, not a market outcome: {:?}",
        output.scenarios
    );
}

// ---------------------------------------------------------------------------
// 3. baseline A + robustness factory B => fail closed
// ---------------------------------------------------------------------------

#[test]
fn robustness_gauntlet_mismatched_factory_fails_closed() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_robustness_gauntlet(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-b")));

    assert!(
        !output.all_applicable_passed(),
        "a robustness gauntlet built from a semantically mismatched factory must never pass"
    );

    // Every scenario that actually invokes `make_strategy()` must attribute
    // its failure to the identity mismatch -- month_year_regime_concentration
    // never calls make_strategy (pure equity-curve analysis) and
    // symbol_leave_one_out is inapplicable for this single-symbol fixture,
    // so both are excluded from this check.
    let factory_driven_names = [
        "execution_delay_stress",
        "parameter_neighborhood_execution",
        "placebo_temporal_offset",
        "conservative_capacity_stress",
    ];
    for name in factory_driven_names {
        let scenario = output
            .scenarios
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("scenario {name} must be present"));
        assert!(
            !scenario.passed,
            "{name} must fail closed on a mismatched factory: {scenario:?}"
        );
        assert!(
            reason_contains_mismatch(&scenario.reason),
            "{name} must attribute its failure to the identity mismatch: {scenario:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. one factory invocation returns B among otherwise-A instances => fail closed
// ---------------------------------------------------------------------------

#[test]
fn one_mismatched_invocation_among_matching_ones_fails_closed() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    // Every call returns "fp-a" EXCEPT the 3rd call, which returns "fp-b" --
    // proves the check is applied per-invocation, not sampled once and
    // assumed representative (the factory contract allows a new instance
    // each call).
    let call_count = Cell::new(0u32);
    let make_strategy = || {
        let n = call_count.get() + 1;
        call_count.set(n);
        let fp = if n == 3 { "fp-b" } else { "fp-a" };
        Box::new(FingerprintedStrategy::new(fp)) as Box<dyn Strategy>
    };

    let output = run_robustness_gauntlet(&baseline, &config, &bars, make_strategy);

    assert!(
        call_count.get() >= 3,
        "test setup requires at least 3 make_strategy() invocations to exercise the 3rd-call mismatch"
    );
    assert!(
        !output.all_applicable_passed(),
        "a single mismatched invocation among otherwise-matching ones must still fail closed: {:?}",
        output.scenarios
    );
    assert!(
        output.scenarios.iter().any(|s| reason_contains_mismatch(&s.reason)),
        "at least one scenario must attribute its failure to the identity mismatch: {:?}",
        output.scenarios
    );
}

// ---------------------------------------------------------------------------
// 5. DelayedStrategy preserves the underlying candidate's real semantic
//    fingerprint -- verification stays about the candidate, not the
//    execution-delay stress decorator.
// ---------------------------------------------------------------------------

#[test]
fn delayed_strategy_scenarios_check_underlying_candidate_not_the_decorator() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    // Matching factory: execution_delay_stress and placebo_temporal_offset
    // both wrap `make_strategy()` in `DelayedStrategy` -- if the decorator's
    // own (spec-only) default fingerprint were used instead of forwarding
    // the wrapped strategy's real fingerprint, these would incorrectly
    // report an identity mismatch even for a genuinely matching candidate.
    let matching =
        run_robustness_gauntlet(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-a")));
    for name in ["execution_delay_stress", "placebo_temporal_offset"] {
        let scenario = matching.scenarios.iter().find(|s| s.name == name).unwrap();
        assert!(
            !reason_contains_mismatch(&scenario.reason),
            "{name} must not false-positive an identity mismatch through the DelayedStrategy wrapper: {scenario:?}"
        );
    }

    // Mismatched factory: the same two scenarios must still catch a real
    // mismatch through the wrapper, proving the delegation is genuinely
    // checked, not merely never-triggered.
    let mismatched =
        run_robustness_gauntlet(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-b")));
    for name in ["execution_delay_stress", "placebo_temporal_offset"] {
        let scenario = mismatched.scenarios.iter().find(|s| s.name == name).unwrap();
        assert!(
            !scenario.passed && reason_contains_mismatch(&scenario.reason),
            "{name} must catch a real mismatch through the DelayedStrategy wrapper: {scenario:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. parameter-neighborhood/sweep path cannot bypass the identity check
// ---------------------------------------------------------------------------

#[test]
fn parameter_neighborhood_path_cannot_bypass_identity_check() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_robustness_gauntlet(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-b")));

    let scenario = output
        .scenarios
        .iter()
        .find(|s| s.name == "parameter_neighborhood_execution")
        .unwrap();
    assert!(
        !scenario.passed,
        "parameter-neighborhood/sweep path must not become a bypass for the identity check: {scenario:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. result-equivalent A/B still rejected if semantic fingerprints differ
// ---------------------------------------------------------------------------

#[test]
fn result_equivalent_instances_still_rejected_on_fingerprint_difference() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    // "fp-b" strategy runs the EXACT same on_bar behavior (hold-flat) as
    // "fp-a" -- their BacktestReport results (equity curve, fills, orders)
    // are bit-identical. Only the declared semantic fingerprint differs.
    // The check below must reject this on identity alone, never by
    // comparing (and finding no difference in) result values.
    let output =
        run_backtest_stress_suite(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-b")));

    assert!(
        !output.all_passed(),
        "identical results must not launder a real semantic fingerprint mismatch into a pass: {:?}",
        output.scenarios
    );
    assert!(
        output.scenarios.iter().all(|s| reason_contains_mismatch(&s.reason)),
        "rejection must be attributed to identity, not to a (nonexistent) result difference: {:?}",
        output.scenarios
    );
}

// ---------------------------------------------------------------------------
// 8. existing genuine stress/robustness positive path stays green
// ---------------------------------------------------------------------------

#[test]
fn genuine_matching_candidate_stress_suite_still_passes() {
    let config = BacktestConfig::test_defaults();
    let bars = bars();
    let baseline = baseline_report(&config, &bars, "fp-a");

    let output =
        run_backtest_stress_suite(&baseline, &config, &bars, || Box::new(FingerprintedStrategy::new("fp-a")));

    assert!(
        output.all_passed(),
        "the identity-binding check must not regress a genuinely matching, otherwise-passing \
         candidate: {:?}",
        output.scenarios
    );
}
