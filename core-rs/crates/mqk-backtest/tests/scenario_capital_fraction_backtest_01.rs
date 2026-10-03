//! FixedInitialCapitalFractionV1 through the real BacktestEngine: the
//! decision entering Backtest carries the policy-resolved quantity, resolved
//! once at the causal close, held, and re-resolved only after a real exit.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestReport,
    QuantitySemanticsId, SizingPolicy, StrategySizingConfig,
};
use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const M: i64 = 1_000_000;
const DAY: i64 = 86_400;
const CAPITAL: i64 = 100_000 * M;

/// Emits +1 share when `script[tick]` is true, otherwise flat.
struct Scripted {
    script: Vec<bool>,
    idx: usize,
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

fn config(bps: Option<i64>) -> BacktestConfig {
    let mut c = BacktestConfig::test_defaults();
    c.timeframe_secs = DAY;
    c.initial_cash_micros = CAPITAL;
    c.max_gross_exposure_mult_micros = 100_000_000;
    c.integrity_enabled = false;
    if let Some(b) = bps {
        c.sizing_policy = SizingPolicy::capital_fraction_v1(b).unwrap();
    }
    c
}

fn run(cfg: BacktestConfig, script: Vec<bool>, bars: &[BacktestBar]) -> BacktestReport {
    let mut e = BacktestEngine::new(cfg);
    e.add_strategy(Box::new(Scripted { script, idx: 0 }))
        .unwrap();
    e.run(bars).unwrap()
}

// open/close pairs per bar; decision at bar i fills at bar i+1's open.
fn bars_a() -> Vec<BacktestBar> {
    vec![
        bar(0, 100, 100),
        bar(1, 100, 100), // entry decision: close 100 -> budget 25_000 -> Q=250
        bar(2, 111, 200), // fill price 111 (NOT the sizing price); close jumps
        bar(3, 200, 250),
        bar(4, 250, 250),
    ]
}

#[test]
fn entry_resolves_at_causal_close_not_fill_price_or_future_close() {
    let r = run(
        config(Some(2_500)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    assert_eq!(r.fills.len(), 1);
    assert_eq!(
        r.fills[0].inner.qty.raw(),
        250 * M,
        "25% of 100k at close 100"
    );
    let e = &r.sizing_provenance.entries;
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].causal_reference_price_micros, 100 * M);
    assert_eq!(e[0].reference_bar_end_ts, DAY * 2);
    assert_eq!(e[0].position_budget_micros, 25_000 * M);
    assert_eq!(e[0].resolved_target_qty_micros, 250 * M);
    assert_eq!(e[0].initial_allocated_capital_micros, CAPITAL);
    assert!(r.sizing_provenance.refusals.is_empty());
}

#[test]
fn quantity_is_held_while_long_and_not_resized_by_price_or_equity() {
    let r = run(
        config(Some(2_500)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    // Price doubled+; a continuous resizer would have traded again.
    assert_eq!(r.fills.len(), 1);
    assert_eq!(r.sizing_provenance.entries.len(), 1);
}

#[test]
fn reentry_re_resolves_from_same_initial_capital_and_new_causal_close() {
    let bars = vec![
        bar(0, 100, 100),
        bar(1, 100, 100), // entry 1 @100 -> Q=250
        bar(2, 100, 100),
        bar(3, 100, 100), // flat decision
        bar(4, 100, 125),
        bar(5, 125, 125), // entry 2 decision @125 -> Q=200
        bar(6, 125, 125),
        bar(7, 125, 125),
    ];
    let r = run(
        config(Some(2_500)),
        vec![false, true, true, false, false, true, true, true],
        &bars,
    );
    let e = &r.sizing_provenance.entries;
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].resolved_target_qty_micros, 250 * M);
    assert_eq!(
        e[1].resolved_target_qty_micros,
        200 * M,
        "25_000/125, from INITIAL capital"
    );
    assert_eq!(e[1].initial_allocated_capital_micros, CAPITAL);
    assert_eq!(e[1].causal_reference_price_micros, 125 * M);
}

#[test]
fn insufficient_budget_refuses_and_stays_flat_never_one_share() {
    // 1 bps of 100k = 10 USD budget; price 100 -> cannot buy a share.
    let r = run(
        config(Some(1)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    assert!(r.fills.is_empty(), "no +1 fallback");
    assert!(r.sizing_provenance.entries.is_empty());
    assert!(!r.sizing_provenance.refusals.is_empty());
    assert_eq!(
        r.sizing_provenance.refusals[0].reason_code,
        "insufficient_budget_for_minimum_quantity"
    );
}

#[test]
fn caps_only_reduce_and_use_the_one_cap_engine() {
    let mut cfg = config(Some(2_500));
    cfg.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: Some(40),
        max_position_notional_usd: None,
    };
    let r = run(cfg, vec![false, true, true, true, true], &bars_a());
    assert_eq!(r.fills[0].inner.qty.raw(), 40 * M);
    let e = &r.sizing_provenance.entries[0];
    assert_eq!(e.uncapped_target_qty_micros, 250 * M);
    assert_eq!(e.capped_by, "max_qty");
}

#[test]
fn future_bars_cannot_change_the_resolved_entry() {
    let a = run(
        config(Some(2_500)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    let mut altered = bars_a();
    altered[3] = bar(3, 1, 1);
    altered[4] = bar(4, 1, 1);
    let b = run(
        config(Some(2_500)),
        vec![false, true, true, true, true],
        &altered,
    );
    assert_eq!(a.sizing_provenance.entries, b.sizing_provenance.entries);
}

#[test]
fn legacy_policy_is_unchanged_one_share_and_default_provenance() {
    let r = run(config(None), vec![false, true, true, true, true], &bars_a());
    assert_eq!(r.fills.len(), 1);
    assert_eq!(r.fills[0].inner.qty.raw(), M, "legacy +1 share");
    assert_eq!(r.sizing_provenance, Default::default());
    assert_eq!(r.sizing_provenance.policy, SizingPolicy::FixedQuantityV1);
}

#[test]
fn capital_fraction_report_and_identity_differ_from_legacy() {
    let legacy = run(config(None), vec![false, true, true, true, true], &bars_a());
    let cf = run(
        config(Some(2_500)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    assert_ne!(legacy.config_id, cf.config_id);
    assert_ne!(legacy.run_id, cf.run_id);
    assert_ne!(
        legacy.strategy_semantic_fingerprint,
        cf.strategy_semantic_fingerprint
    );
    let other = run(
        config(Some(2_501)),
        vec![false, true, true, true, true],
        &bars_a(),
    );
    assert_ne!(
        cf.strategy_semantic_fingerprint,
        other.strategy_semantic_fingerprint
    );
}

#[test]
fn invalid_policy_and_unsupported_semantics_fail_closed_before_any_bar() {
    let mut cfg = config(None);
    cfg.sizing_policy = SizingPolicy::FixedInitialCapitalFractionV1 {
        allocation_fraction_bps: 0,
    };
    let mut e = BacktestEngine::new(cfg);
    let err = e
        .add_strategy(Box::new(Scripted {
            script: vec![],
            idx: 0,
        }))
        .unwrap_err();
    assert!(matches!(err, BacktestError::InvalidSizingPolicy { .. }));

    let mut cfg = config(Some(2_500));
    cfg.quantity_semantics = QuantitySemanticsId::FractionalQtyMicrosV1;
    let mut e = BacktestEngine::new(cfg);
    let err = e
        .add_strategy(Box::new(Scripted {
            script: vec![],
            idx: 0,
        }))
        .unwrap_err();
    assert!(matches!(err, BacktestError::InvalidSizingPolicy { .. }));
}

mod scanner_agreement {
    use mqk_backtest::{
        execute_strategy_scan_with_policy, load_csv_file, BacktestConfig, BacktestEngine,
        ScanBenchmarkPolicy, ScanRunRequest, SizingPolicy, StrategyScanPolicy,
    };
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    use mqk_strategy::PluginRegistry;

    const DAY: i64 = 86_400;
    const STRATEGY: &str = "absolute_momentum_252";

    fn fixture(tag: &str) -> (std::path::PathBuf, ScanRunRequest, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!("mqk_cf_scan_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("bars").join("1D")).unwrap();
        std::fs::write(
            p.join("registry.json"),
            r#"[{"instrument_id":"equity:US:SPY","symbol":"SPY","asset_class":"equity","provider":"alpaca","provider_symbol":"SPY","venue":"NYSE","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"fixture"}]"#,
        )
        .unwrap();
        let mut csv =
            String::from("symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume\n");
        for i in 0..320i64 {
            let c = 100_000_000 + i * 50_000;
            csv.push_str(&format!(
                "SPY,{},{},{},{},{},1000\n",
                DAY * (i + 1),
                c,
                c + 1_000_000,
                c - 1_000_000,
                c
            ));
        }
        let bars_path = p.join("bars").join("1D").join("SPY_1D.csv");
        std::fs::write(&bars_path, csv).unwrap();
        let req = ScanRunRequest {
            registry_path: p.join("registry.json").display().to_string(),
            bars_root: p.join("bars").display().to_string(),
            timeframe: "1D".to_string(),
            strategies: vec![STRATEGY.to_string()],
            top: 5,
            limit_symbols: None,
            git_hash: "TEST".to_string(),
            created_at_utc: "2026-10-03T00:00:00Z".to_string(),
        };
        (p, req, bars_path)
    }

    fn cfg(bps: Option<i64>) -> BacktestConfig {
        let mut c = BacktestConfig::conservative_defaults();
        c.timeframe_secs = DAY;
        c.integrity_enabled = false;
        if let Some(b) = bps {
            c.sizing_policy = SizingPolicy::capital_fraction_v1(b).unwrap();
        }
        c
    }

    fn policy(c: &BacktestConfig, b: ScanBenchmarkPolicy) -> StrategyScanPolicy {
        StrategyScanPolicy {
            base_config: c.clone(),
            benchmark_policy: b,
            ..StrategyScanPolicy::default()
        }
    }

    #[test]
    fn scanner_and_direct_backtest_agree_on_capital_fraction_sizing() {
        let (dir, req, bars_path) = fixture("agree");
        let c = cfg(Some(2_500));
        let scan = execute_strategy_scan_with_policy(
            &req,
            policy(&c, ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1),
        )
        .unwrap();
        let cand = &scan.candidates[0];

        let bars = load_csv_file(&bars_path).unwrap();
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
        let mut engine = BacktestEngine::new(c.clone());
        engine
            .add_strategy(reg.instantiate(STRATEGY).unwrap())
            .unwrap();
        let direct = engine.run(&bars).unwrap();

        assert!(!direct.sizing_provenance.entries.is_empty());
        assert_eq!(cand.metrics.fill_count, Some(direct.fills.len()));
        let q = direct.sizing_provenance.entries[0].resolved_target_qty_micros;
        assert!(
            q > 1_000_000,
            "capital-fraction Q must exceed legacy +1 share"
        );
        assert_eq!(direct.fills[0].inner.qty.raw(), q);
        let ev = cand
            .metrics
            .benchmark_capital_fraction
            .as_ref()
            .expect("capital-fraction benchmark evidence");
        assert_eq!(ev.candidate_target_qty_micros, q);
        assert_eq!(ev.benchmark_target_qty_micros, q);

        let legacy = execute_strategy_scan_with_policy(
            &req,
            policy(&cfg(None), ScanBenchmarkPolicy::LegacyFullyInvested),
        )
        .unwrap();
        assert_ne!(
            scan.scan_id, legacy.scan_id,
            "scan identity binds sizing policy"
        );
        assert_ne!(
            cand.metrics.total_return_pct,
            legacy.candidates[0].metrics.total_return_pct
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn capital_fraction_config_under_non_cf_benchmark_policy_is_refused() {
        let (dir, req, _) = fixture("refuse");
        let c = cfg(Some(2_500));
        for b in [
            ScanBenchmarkPolicy::LegacyFullyInvested,
            ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ] {
            assert!(execute_strategy_scan_with_policy(&req, policy(&c, b)).is_err());
        }
        // and the converse: CF benchmark policy over a legacy fixed-quantity config
        assert!(execute_strategy_scan_with_policy(
            &req,
            policy(
                &cfg(None),
                ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1
            ),
        )
        .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
