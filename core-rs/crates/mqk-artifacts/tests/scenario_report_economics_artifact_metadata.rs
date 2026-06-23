//! BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: proves that `write_backtest_report`
//! surfaces truthful instrument economics in `metrics.json` and `report.md`,
//! for both default (equity) and synthetic multiplier runs.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestInstrumentEconomics};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};
use std::fs;

const M: i64 = 1_000_000;

struct BuyHoldSell {
    bar_idx: u64,
    qty: i64,
}

impl BuyHoldSell {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyHoldSell", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 | 2 => StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)]),
            _ => StrategyOutput::new(vec![TargetPosition::new("ES", 0)]),
        }
    }
}

fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
}

fn buy_hold_sell_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 4_500),
        flat_bar(1_700_000_120, 4_505),
        flat_bar(1_700_000_180, 4_510),
        flat_bar(1_700_000_240, 4_515),
    ]
}

fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity
    cfg
}

/// Run the engine (optionally with economics) and write artifacts to a fresh
/// temp dir. Returns (metrics.json contents, report.md contents).
fn run_and_write(label: &str, economics: Option<BacktestInstrumentEconomics>) -> (String, String) {
    let bars = buy_hold_sell_bars();
    let cfg = cfg_with_wide_cap();
    let initial_cash = cfg.initial_cash_micros;

    let mut engine = BacktestEngine::new(cfg);
    if let Some(econ) = economics {
        engine = engine.with_economics(econ);
    }
    engine.add_strategy(Box::new(BuyHoldSell::new(10))).unwrap();
    let report = engine.run(&bars).expect("engine.run must succeed");

    let tmp = std::env::temp_dir().join(format!(
        "mqk_brea03_{}_{}",
        label,
        std::process::id()
    ));
    fs::create_dir_all(&tmp).unwrap();

    mqk_artifacts::write_backtest_report(&tmp, &report, initial_cash)
        .expect("write_backtest_report must succeed");

    let metrics_json = fs::read_to_string(tmp.join("metrics.json")).expect("metrics.json");
    let report_md = fs::read_to_string(tmp.join("report.md")).expect("report.md");
    let _ = fs::remove_dir_all(&tmp);

    (metrics_json, report_md)
}

/// brea03a: default (equity) run reports truthful equity economics in metrics.json.
#[test]
fn brea03a_default_equity_metrics_json_economics_is_truthful() {
    let (metrics_json, report_md) = run_and_write("default", None);
    let v: serde_json::Value =
        serde_json::from_str(&metrics_json).expect("metrics.json must be valid JSON");

    let econ = &v["economics"];
    assert!(!econ.is_null(), "economics object must be present");
    assert_eq!(econ["contract_multiplier"], 1);
    assert!(econ["initial_margin_micros"].is_null());
    assert!(econ["maintenance_margin_micros"].is_null());
    assert_eq!(econ["margin_enforced"], false);
    // Strategy actually traded: (4,510 - 4,500) * 10 = $100 realized.
    assert_eq!(econ["realized_pnl_micros"], 100 * M);

    assert!(
        report_md.contains("## Instrument Economics"),
        "report.md must contain an Instrument Economics section"
    );
    assert!(report_md.contains("Contract Multiplier | 1"));
}

/// brea03b: synthetic multiplier=50 run reports truthful multiplier-aware
/// economics in both metrics.json and report.md.
#[test]
fn brea03b_multiplier_50_metrics_json_economics_is_truthful() {
    let econ50 = BacktestInstrumentEconomics::new(50, Some(10_000 * M), Some(5_000 * M)).unwrap();
    let (metrics_json, report_md) = run_and_write("mult50", Some(econ50));
    let v: serde_json::Value =
        serde_json::from_str(&metrics_json).expect("metrics.json must be valid JSON");

    let econ = &v["economics"];
    assert_eq!(econ["contract_multiplier"], 50);
    assert_eq!(econ["initial_margin_micros"], 10_000 * M);
    assert_eq!(econ["maintenance_margin_micros"], 5_000 * M);
    assert_eq!(econ["margin_enforced"], false, "margin metadata must never claim enforcement");
    // Realized P&L scales by the multiplier: $100 * 50 = $5,000.
    assert_eq!(econ["realized_pnl_micros"], 50 * 100 * M);

    assert!(report_md.contains("## Instrument Economics"));
    assert!(report_md.contains("Contract Multiplier | 50"));
    assert!(report_md.contains("Margin Enforced | false"));
    assert!(
        report_md.contains("Multiplier economics active"),
        "report.md must call out multiplier-aware equity curve for non-default economics"
    );
}

/// brea03c: schema_version stays 1 -- economics is an additive metrics.json field.
#[test]
fn brea03c_metrics_json_schema_version_unchanged() {
    let (metrics_json, _) = run_and_write("schema", None);
    let v: serde_json::Value = serde_json::from_str(&metrics_json).unwrap();
    assert_eq!(v["schema_version"], 1, "economics must be additive, not a schema bump");
}
