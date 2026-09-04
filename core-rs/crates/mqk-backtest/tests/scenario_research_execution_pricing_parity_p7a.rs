//! P7A (RESEARCH-EXECUTION-PRICING-PARITY-01) cross-language golden proof.
//!
//! Loads the SAME golden vector consumed by
//! research-py/tests/test_execution_pricing_parity_p7a.py
//! (docs/research/fixtures/execution_pricing_golden_v1.json) and runs each
//! case through the REAL, unmodified `BacktestEngine` (not a reimplemented
//! formula) to prove the accepted Rust conservative fill model actually
//! produces the fixture's `expected_fill_price_micros` -- the same value
//! the Python side asserts its `conservative_fill_price_micros` port
//! produces for the identical inputs. Neither language calls the other at
//! runtime; the fixture is the shared, static proof (per the mission's
//! "do not call one language from the other at runtime" instruction).

use std::path::Path;

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, StressProfile};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const SYMBOL: &str = "GOLD";

fn to_micros(x: f64) -> i64 {
    (x * 1_000_000.0).round() as i64
}

#[derive(serde::Deserialize)]
struct GoldenCase {
    name: String,
    high: f64,
    low: f64,
    close: f64,
    side: String,
    slippage_bps: i64,
    volatility_mult_bps: i64,
    expected_fill_price_micros: i64,
}

#[derive(serde::Deserialize)]
struct GoldenFixture {
    cases: Vec<GoldenCase>,
}

fn load_fixture() -> GoldenFixture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/research/fixtures/execution_pricing_golden_v1.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read golden fixture at {:?}: {e}", path));
    serde_json::from_str(&text).expect("golden fixture must be valid JSON")
}

/// Arbitrary, unmeasured setup bar -- only its presence (not its price)
/// matters; it exists solely so a later bar's order has "an earlier bar of
/// this symbol" to be signalled from (BKT-FUTURE-EXECUTION-01: signal at T,
/// fill only on the first strictly-later bar).
fn setup_bar(ts: i64) -> BacktestBar {
    BacktestBar::new(SYMBOL, ts, 500_000_000, 500_000_000, 500_000_000, 500_000_000, 1000)
}

fn golden_bar(ts: i64, case: &GoldenCase) -> BacktestBar {
    let h = to_micros(case.high);
    let l = to_micros(case.low);
    let c = to_micros(case.close);
    BacktestBar::new(SYMBOL, ts, c, h, l, c, 1000)
}

/// Strategy that emits a target-quantity change at specific 1-indexed bar
/// positions (mirrors the repo's existing FlipOnce test-strategy pattern --
/// see scenario_volatility_slippage_scales_with_spread.rs) and does nothing
/// otherwise.
struct ScriptedStrategy {
    bar_idx: u64,
    plan: Vec<(u64, i64)>,
}

impl ScriptedStrategy {
    fn new(plan: Vec<(u64, i64)>) -> Self {
        Self { bar_idx: 0, plan }
    }
}

impl Strategy for ScriptedStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Scripted", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        for (idx, qty) in &self.plan {
            if *idx == self.bar_idx {
                return StrategyOutput::new(vec![TargetPosition::new(SYMBOL, *qty)]);
            }
        }
        StrategyOutput::new(vec![])
    }
}

#[test]
fn golden_vector_matches_real_engine_fill_prices() {
    let fixture = load_fixture();
    assert!(
        !fixture.cases.is_empty(),
        "golden fixture must declare at least one case"
    );

    let ts_base = 1_700_000_000_i64;
    for case in &fixture.cases {
        let mut cfg = BacktestConfig::test_defaults();
        cfg.stress = StressProfile {
            slippage_bps: case.slippage_bps,
            volatility_mult_bps: case.volatility_mult_bps,
            participation_impact_bps: 0,
        };

        let (bars, plan, expected_fill_count) = match case.side.as_str() {
            "buy" => (
                vec![setup_bar(ts_base), golden_bar(ts_base + 60, case)],
                vec![(1_u64, 1_i64)],
                1usize,
            ),
            "sell" => (
                vec![
                    setup_bar(ts_base),
                    setup_bar(ts_base + 60),
                    golden_bar(ts_base + 120, case),
                ],
                vec![(1_u64, 1_i64), (2_u64, 0_i64)],
                2usize,
            ),
            other => panic!("golden case {:?}: unsupported side {other:?}", case.name),
        };

        let mut engine = BacktestEngine::new(cfg);
        engine
            .add_strategy(Box::new(ScriptedStrategy::new(plan)))
            .unwrap();
        let report = engine.run(&bars).unwrap();

        assert!(
            !report.halted,
            "golden case {:?}: engine must not halt",
            case.name
        );
        assert_eq!(
            report.fills.len(),
            expected_fill_count,
            "golden case {:?}: unexpected fill count",
            case.name
        );

        let measured = report.fills.last().unwrap();
        assert_eq!(
            measured.price_micros, case.expected_fill_price_micros,
            "golden case {:?}: Rust conservative_fill_price produced {} micros, \
             fixture expects {} micros",
            case.name, measured.price_micros, case.expected_fill_price_micros
        );
    }
}
