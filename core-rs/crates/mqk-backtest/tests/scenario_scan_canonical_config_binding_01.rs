//! IR-BV2-01 scanner side: a Benchmark V2 scan run under the canonical
//! Backtest config (`execute_strategy_scan_with_policy`) must produce
//! candidate evidence whose run identity is exactly that of a real canonical
//! Backtest run over the same bars, and the scan identity must bind the
//! config so differently-configured scans can never share an artifact dir.

use mqk_backtest::{
    execute_strategy_scan_with_benchmark, execute_strategy_scan_with_policy, load_csv_file,
    BacktestConfig, BacktestEngine, ScanBenchmarkPolicy, ScanRunOutput, ScanRunRequest,
    StrategyScanPolicy,
};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::PluginRegistry;

const DAY: i64 = 86_400;
const STRATEGY: &str = "absolute_momentum_252";

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// registry + one SPY 1D bars file laid out the way `scan-strategies` reads.
fn fixture() -> (TmpDir, ScanRunRequest, std::path::PathBuf) {
    // Unique per fixture: tests in this binary run in parallel.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("mqk_scan_canon_cfg_{}_{seq}", std::process::id()));
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
    (TmpDir(p), req, bars_path)
}

fn canonical_cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = DAY;
    cfg.integrity_enabled = true;
    cfg.integrity_stale_threshold_ticks = 259_200;
    cfg.integrity_gap_tolerance_bars = 3;
    cfg
}

fn v2_policy(cfg: &BacktestConfig) -> StrategyScanPolicy {
    StrategyScanPolicy {
        base_config: cfg.clone(),
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanPolicy::default()
    }
}

fn evidence(out: &ScanRunOutput) -> &mqk_backtest::ScanBenchmarkV2Evidence {
    out.candidates[0]
        .metrics
        .benchmark_v2
        .as_ref()
        .unwrap_or_else(|| panic!("V2 evidence expected: {:?}", out.candidates[0]))
}

/// The canonical Backtest run over the same bars (what `backtest csv` runs).
fn canonical_run(
    cfg: &BacktestConfig,
    bars_path: &std::path::Path,
) -> mqk_backtest::BacktestReport {
    let bars = load_csv_file(bars_path).unwrap();
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let mut engine = BacktestEngine::new(cfg.clone());
    engine
        .add_strategy(reg.instantiate(STRATEGY).unwrap())
        .unwrap();
    engine.run(&bars).unwrap()
}

#[test]
fn canonical_policy_scan_evidence_has_the_canonical_backtest_run_identity() {
    let (_g, req, bars_path) = fixture();
    let cfg = canonical_cfg();
    let out = execute_strategy_scan_with_policy(&req, v2_policy(&cfg)).unwrap();
    let ev = evidence(&out);
    let report = canonical_run(&cfg, &bars_path);
    assert_eq!(ev.candidate_config_id, report.config_id.to_string());
    assert_eq!(ev.candidate_run_id, report.run_id.to_string());
    assert_eq!(ev.input_data_hash, report.input_data_hash);
    assert_eq!(ev.candidate_execution_model_id, report.execution_model_id);
}

/// Controller for IR-BV2-01 on the production scan path: the DEFAULT V2 scan
/// (scanner-default config) does NOT carry the canonical run identity.
#[test]
fn default_policy_v2_scan_does_not_carry_the_canonical_run_identity() {
    let (_g, req, bars_path) = fixture();
    let out = execute_strategy_scan_with_benchmark(
        &req,
        ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
    )
    .unwrap();
    let ev = evidence(&out);
    let report = canonical_run(&canonical_cfg(), &bars_path);
    assert_ne!(ev.candidate_config_id, report.config_id.to_string());
    assert_ne!(ev.candidate_run_id, report.run_id.to_string());
}

#[test]
fn scan_identity_binds_the_base_config() {
    let (_g, req, _bars) = fixture();
    let cfg = canonical_cfg();
    let a = execute_strategy_scan_with_policy(&req, v2_policy(&cfg)).unwrap();
    let a_again = execute_strategy_scan_with_policy(&req, v2_policy(&cfg)).unwrap();
    assert_eq!(a.scan_id, a_again.scan_id, "same config -> same scan_id");

    let mut other = cfg.clone();
    other.commission.per_share_micros += 1_000;
    let b = execute_strategy_scan_with_policy(&req, v2_policy(&other)).unwrap();
    assert_ne!(
        a.scan_id, b.scan_id,
        "different config -> different scan_id"
    );

    let default_v2 = execute_strategy_scan_with_benchmark(
        &req,
        ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
    )
    .unwrap();
    assert_ne!(a.scan_id, default_v2.scan_id);

    let legacy =
        execute_strategy_scan_with_benchmark(&req, ScanBenchmarkPolicy::LegacyFullyInvested)
            .unwrap();
    assert_ne!(a.scan_id, legacy.scan_id);
    assert!(legacy.manifest.benchmark_policy_id.is_none());
}
