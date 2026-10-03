//! Promotion binding for capital-fraction native evidence: a
//! `FixedInitialCapitalFractionV1` canonical Backtest run requires a review
//! judged against the capital-fraction-matched passive benchmark, bound to the
//! same config/run/capital/data/sizing parameters/first resolved entry. Both
//! sides come from the real scanner -> review -> artifact -> validation path
//! and a real `BacktestEngine` run.

use mqk_backtest::{
    evaluate_scan_candidate, evaluate_scan_candidate_with_emission, execute_strategy_scan_review,
    write_review_artifacts, write_scan_artifacts, BacktestBar, BacktestConfig, BacktestEngine,
    BacktestReport, ReviewRunRequest, ScanBenchmarkPolicy, ScanManifest, ScanRunOutput,
    ScanSummary, SizingPolicy, StrategyScanCandidate, StrategyScanPolicy, StrategyScanReviewPolicy,
    StrategyScanReviewState,
};
use mqk_daemon::promotion_evidence_validation::{
    enforce_native_review_benchmark_binding, validate_paper_candidate_evidence,
    BacktestEvidenceIdentity, ValidatedEvidence,
};
use mqk_daemon::state::{AppState, OperatorAuthMode};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy};

const DAY: i64 = 86_400;
const STRATEGY: &str = "absolute_momentum_252";
const BPS: i64 = 2_500;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn instance() -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(STRATEGY).unwrap()
}

fn bars_with_slope(slope: i64) -> Vec<BacktestBar> {
    (0..320i64)
        .map(|i| {
            let c = 100_000_000 + i * slope;
            BacktestBar::new(
                "SPY",
                DAY * (i + 1),
                c,
                c + 1_000_000,
                c - 1_000_000,
                c,
                1_000,
            )
        })
        .collect()
}

fn bars() -> Vec<BacktestBar> {
    bars_with_slope(50_000)
}

fn canonical_cfg(policy: SizingPolicy) -> BacktestConfig {
    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = DAY;
    cfg.integrity_enabled = true;
    cfg.integrity_stale_threshold_ticks = 259_200;
    cfg.integrity_gap_tolerance_bars = 3;
    cfg.sizing_policy = policy;
    cfg
}

fn cf_cfg(bps: i64) -> BacktestConfig {
    canonical_cfg(SizingPolicy::capital_fraction_v1(bps).unwrap())
}

fn cf_candidate(cfg: &BacktestConfig, bars: &[BacktestBar]) -> StrategyScanCandidate {
    let cand = evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(bars),
        &StrategyScanPolicy {
            base_config: cfg.clone(),
            benchmark_policy: ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
            ..StrategyScanPolicy::default()
        },
    );
    assert!(
        cand.metrics.benchmark_capital_fraction.is_some(),
        "fixture candidate must carry capital-fraction evidence: {cand:?}"
    );
    cand
}

fn v2_candidate(cfg: &BacktestConfig, bars: &[BacktestBar]) -> StrategyScanCandidate {
    let cand = evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(instance()),
        Some(bars),
        &StrategyScanPolicy {
            base_config: cfg.clone(),
            benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
            ..StrategyScanPolicy::default()
        },
    );
    assert!(cand.metrics.benchmark_v2.is_some());
    cand
}

fn backtest_report(cfg: &BacktestConfig, bars: &[BacktestBar]) -> BacktestReport {
    let mut engine = BacktestEngine::new(cfg.clone());
    engine.add_strategy(instance()).unwrap();
    engine.run(bars).expect("canonical backtest run")
}

struct Fx {
    _dir: tempfile::TempDir,
    st: AppState,
    review_dir: String,
}

fn fixture(candidate: StrategyScanCandidate, benchmark: ScanBenchmarkPolicy) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let manifest = ScanManifest {
        schema_version: 1,
        scan_id: "scan-fixture".to_string(),
        created_at_utc: "2026-10-03T00:00:00Z".to_string(),
        git_hash: "TEST".to_string(),
        registry_path: "registry.json".to_string(),
        bars_root: "bars".to_string(),
        timeframe: "1D".to_string(),
        strategies: vec![STRATEGY.to_string()],
        universe_count: 1,
        ranked_count: 1,
        skipped_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
        benchmark_policy_id: benchmark.policy_id().map(str::to_string),
    };
    let summary = ScanSummary {
        scan_id: "scan-fixture".to_string(),
        universe_count: 1,
        ranked_count: 1,
        skipped_count: 0,
        top_ranked: Vec::new(),
        top_skip_reasons: Vec::new(),
    };
    let scan_out = ScanRunOutput {
        scan_id: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"fixture-cf"),
        manifest,
        candidates: vec![candidate],
        summary,
    };
    let scan_dir = write_scan_artifacts(&dir.path().join("scan"), &scan_out).unwrap();
    let mut out = execute_strategy_scan_review(&ReviewRunRequest {
        artifact_dir: scan_dir.display().to_string(),
        top: 5,
        policy: StrategyScanReviewPolicy {
            benchmark_policy: benchmark,
            ..StrategyScanReviewPolicy::default()
        },
        git_hash: "TEST".to_string(),
        created_at_utc: "2026-10-03T00:00:00Z".to_string(),
    })
    .unwrap();
    assert_ne!(
        out.decisions[0].review_state,
        StrategyScanReviewState::Blocked,
        "fixture row must not be Blocked: {:?}",
        out.decisions[0]
    );
    // The binding never looks at classification; force the state it requires.
    out.decisions[0].review_state = StrategyScanReviewState::PaperCandidate;
    let review_dir = write_review_artifacts(&dir.path().join("reviews"), &out).unwrap();

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", dir.path());
    let st = AppState::new_with_operator_auth(OperatorAuthMode::ExplicitDevNoToken);
    Fx {
        _dir: dir,
        st,
        review_dir: review_dir.display().to_string(),
    }
}

fn validated(fx: &Fx) -> ValidatedEvidence {
    validate_paper_candidate_evidence(&fx.st, &fx.review_dir, STRATEGY, "SPY", DAY)
        .expect("fixture review evidence must validate")
}

fn cf_fixture(cfg: &BacktestConfig, bars: &[BacktestBar]) -> (Fx, ValidatedEvidence) {
    let fx = fixture(
        cf_candidate(cfg, bars),
        ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
    );
    let ev = validated(&fx);
    (fx, ev)
}

fn fingerprint(ev: &ValidatedEvidence) -> String {
    ev.benchmark_capital_fraction
        .as_ref()
        .map(|b| b.strategy_semantic_fingerprint.clone())
        .or_else(|| {
            ev.benchmark_v2
                .as_ref()
                .map(|b| b.strategy_semantic_fingerprint.clone())
        })
        .unwrap()
}

fn promote(ev: &ValidatedEvidence, report: &BacktestReport, capital: i64) -> Result<(), String> {
    promote_with_fp(&fingerprint(ev), ev, report, capital)
}

fn promote_with_fp(
    fp: &str,
    ev: &ValidatedEvidence,
    report: &BacktestReport,
    capital: i64,
) -> Result<(), String> {
    let identity = BacktestEvidenceIdentity::from_report(report, capital);
    enforce_native_review_benchmark_binding(Some(fp), Some(ev), STRATEGY, "SPY", DAY, &identity)
}

#[test]
fn genuine_capital_fraction_review_on_matching_canonical_run_is_accepted() {
    let bars = bars();
    let cfg = cf_cfg(BPS);
    let (_fx, ev) = cf_fixture(&cfg, &bars);
    let report = backtest_report(&cfg, &bars);
    assert!(report.sizing_provenance.policy.is_capital_fraction());
    let b = ev.benchmark_capital_fraction.as_ref().unwrap();
    assert_eq!(b.candidate_config_id, report.config_id.to_string());
    assert_eq!(b.candidate_run_id, report.run_id.to_string());
    promote(&ev, &report, cfg.initial_cash_micros).expect("matching evidence must be accepted");
}

#[test]
fn review_policy_substitution_is_refused_both_ways() {
    let bars = bars();

    // V2 review row (legacy-sized candidate) against a capital-fraction canonical run.
    let legacy_cfg = canonical_cfg(SizingPolicy::FixedQuantityV1);
    let v2_fx = fixture(
        v2_candidate(&legacy_cfg, &bars),
        ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
    );
    let v2_ev = validated(&v2_fx);
    let cf_report = backtest_report(&cf_cfg(BPS), &bars);
    let legacy_report = backtest_report(&legacy_cfg, &bars);
    promote(&v2_ev, &legacy_report, legacy_cfg.initial_cash_micros)
        .expect("control: V2 review on legacy canonical run");
    let err = promote(&v2_ev, &cf_report, legacy_cfg.initial_cash_micros).unwrap_err();
    assert!(err.contains("capital-fraction"), "{err}");

    // Capital-fraction review row against a legacy canonical run.
    let cfg = cf_cfg(BPS);
    let (_fx, cf_ev) = cf_fixture(&cfg, &bars);
    promote(&cf_ev, &cf_report, cfg.initial_cash_micros).expect("control");
    let err = promote(&cf_ev, &legacy_report, legacy_cfg.initial_cash_micros).unwrap_err();
    assert!(err.contains("Benchmark V2"), "{err}");
}

#[test]
fn canonical_run_with_different_sizing_parameters_or_inputs_is_refused() {
    let bars = bars();
    let cfg = cf_cfg(BPS);
    let (_fx, ev) = cf_fixture(&cfg, &bars);
    let cap = cfg.initial_cash_micros;
    promote(&ev, &backtest_report(&cfg, &bars), cap).expect("control");

    let other_fraction = backtest_report(&cf_cfg(5_000), &bars);
    assert!(promote(&ev, &other_fraction, cap).is_err(), "fraction");

    let mut other_capital_cfg = cfg.clone();
    other_capital_cfg.initial_cash_micros /= 2;
    let other_capital = backtest_report(&other_capital_cfg, &bars);
    assert!(
        promote(&ev, &other_capital, other_capital_cfg.initial_cash_micros).is_err(),
        "capital"
    );

    let other_data = backtest_report(&cfg, &bars_with_slope(60_000));
    assert!(promote(&ev, &other_data, cap).is_err(), "data");

    let mut capped = cfg.clone();
    capped.sizing.max_target_qty = Some(1);
    assert!(
        promote(&ev, &backtest_report(&capped, &bars), cap).is_err(),
        "cap"
    );

    let good = backtest_report(&cfg, &bars);
    let mut no_entries = good.clone();
    no_entries.sizing_provenance.entries.clear();
    assert!(promote(&ev, &no_entries, cap).is_err(), "no entries");

    let mut moved = good.clone();
    moved.sizing_provenance.entries[0].resolved_target_qty_micros += 1_000_000;
    assert!(promote(&ev, &moved, cap).is_err(), "first-entry quantity");

    let mut legacy_claim = good.clone();
    legacy_claim.sizing_provenance.policy = SizingPolicy::FixedQuantityV1;
    assert!(promote(&ev, &legacy_claim, cap).is_err(), "policy claim");

    assert!(promote(&ev, &good, cap / 2).is_err(), "capital basis");
}

type Tamper = fn(&mut ValidatedEvidence);

#[test]
fn tampered_review_evidence_is_refused_field_by_field() {
    let bars = bars();
    let cfg = cf_cfg(BPS);
    let (_fx, good) = cf_fixture(&cfg, &bars);
    let report = backtest_report(&cfg, &bars);
    let cap = cfg.initial_cash_micros;
    promote(&good, &report, cap).expect("control");
    let fp = fingerprint(&good);

    let cases: Vec<(&str, Tamper)> = vec![
        ("fraction", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .allocation_fraction_bps += 1
        }),
        ("capital", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .initial_allocated_capital_micros += 1
        }),
        ("quantity", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .candidate_target_qty_micros += 1_000_000
        }),
        ("reference ts", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .reference_bar_end_ts += DAY
        }),
        ("reference price", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .reference_price_micros += 1
        }),
        ("budget", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .position_budget_micros += 1
        }),
        ("benchmark run id substituted", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .benchmark_run_id =
                uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"other").to_string()
        }),
        ("benchmark entry index", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .benchmark_entry_bar_index += 1
        }),
        ("candidate run id", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .candidate_run_id = uuid::Uuid::nil().to_string()
        }),
        ("candidate config id", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .candidate_config_id = "not-a-uuid".to_string()
        }),
        ("data hash", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .input_data_hash = "0".repeat(64)
        }),
        ("execution model", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .candidate_execution_model_id = "same_bar_v0".to_string()
        }),
        ("evidence policy id", |e| {
            e.benchmark_capital_fraction.as_mut().unwrap().policy_id = "legacy".to_string()
        }),
        ("sizing policy id", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .sizing_policy_id = "fixed_quantity_v1".to_string()
        }),
        ("strategy id", |e| {
            e.benchmark_capital_fraction.as_mut().unwrap().strategy_id = "other".to_string()
        }),
        ("symbol", |e| {
            e.benchmark_capital_fraction.as_mut().unwrap().symbol = "QQQ".to_string()
        }),
        ("fingerprint", |e| {
            e.benchmark_capital_fraction
                .as_mut()
                .unwrap()
                .strategy_semantic_fingerprint = "f".repeat(64)
        }),
        ("score is not the alpha", |e| {
            e.scanner_score = e.scanner_score.map(|s| s + 1.0)
        }),
        ("manifest policy id", |e| {
            e.benchmark_policy_id = Some("capital_matched_exact_target_buy_hold_v1".to_string())
        }),
        ("manifest policy absent", |e| e.benchmark_policy_id = None),
        ("evidence absent", |e| e.benchmark_capital_fraction = None),
    ];
    for (name, mutate) in cases {
        let mut e = good.clone();
        mutate(&mut e);
        let res = promote_with_fp(&fp, &e, &report, cap);
        assert!(res.is_err(), "{name}: must be refused, got {res:?}");
    }
}

#[test]
fn a_substituted_benchmark_run_id_is_refused_for_its_own_reason() {
    let bars = bars();
    let cfg = cf_cfg(BPS);
    let (_fx, good) = cf_fixture(&cfg, &bars);
    let report = backtest_report(&cfg, &bars);
    let cap = cfg.initial_cash_micros;
    let fp = fingerprint(&good);
    promote_with_fp(&fp, &good, &report, cap).expect("control");
    let mut e = good.clone();
    e.benchmark_capital_fraction
        .as_mut()
        .unwrap()
        .benchmark_run_id =
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"another benchmark run").to_string();
    let err = promote_with_fp(&fp, &e, &report, cap).unwrap_err();
    assert!(err.contains("benchmark_run_id does not match"), "{err}");
}

#[test]
fn mixed_benchmark_evidence_and_a_self_consistent_other_fraction_are_refused_with_their_own_reason()
{
    let bars = bars();
    let cfg = cf_cfg(BPS);
    let (_fx, good) = cf_fixture(&cfg, &bars);
    let report = backtest_report(&cfg, &bars);
    let cap = cfg.initial_cash_micros;
    promote(&good, &report, cap).expect("control");
    let fp = fingerprint(&good);

    // Capital-fraction evidence that also carries Benchmark V2 evidence is ambiguous authority.
    let legacy_cfg = canonical_cfg(SizingPolicy::FixedQuantityV1);
    let v2_fx = fixture(
        v2_candidate(&legacy_cfg, &bars),
        ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
    );
    let mut mixed = good.clone();
    mixed.benchmark_v2 = validated(&v2_fx).benchmark_v2;
    assert!(mixed.benchmark_v2.is_some());
    let err = promote_with_fp(&fp, &mixed, &report, cap).unwrap_err();
    assert!(err.contains("carries Benchmark V2 evidence"), "{err}");

    // A different fraction whose budget is recomputed passes internal consistency; only the
    // policy/fraction agreement with the canonical run can refuse it.
    let other = (BPS + 1..BPS + 400)
        .find_map(|bps| {
            let mut e = good.clone();
            let b = e.benchmark_capital_fraction.as_mut().unwrap();
            b.allocation_fraction_bps = bps;
            b.position_budget_micros =
                (b.initial_allocated_capital_micros as i128 * bps as i128 / 10_000) as i64;
            b.verify_internal().is_ok().then_some(e)
        })
        .expect("a self-consistent other fraction exists");
    let err = promote_with_fp(&fp, &other, &report, cap).unwrap_err();
    assert!(err.contains("sizing policy/fraction"), "{err}");
}

#[test]
fn legacy_fixed_quantity_canonical_run_still_requires_benchmark_v2() {
    let bars = bars();
    let cfg = canonical_cfg(SizingPolicy::FixedQuantityV1);
    let report = backtest_report(&cfg, &bars);
    assert_eq!(
        report.sizing_provenance.policy,
        SizingPolicy::FixedQuantityV1
    );
    let (_fx, cf_ev) = cf_fixture(&cf_cfg(BPS), &bars);
    let err = promote(&cf_ev, &report, cfg.initial_cash_micros).unwrap_err();
    assert!(err.contains("Benchmark V2"), "{err}");
}
