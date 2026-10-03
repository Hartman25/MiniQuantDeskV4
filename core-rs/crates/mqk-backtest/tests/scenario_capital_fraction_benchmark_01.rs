//! CapitalFractionMatchedPassiveBuyHoldV1 as scanner/review alpha authority
//! for FixedInitialCapitalFractionV1 candidates. Candidates come from the REAL
//! scanner path; negatives tamper with genuine evidence one field at a time.

use mqk_backtest::benchmark_capital_fraction::{
    compute_capital_fraction_benchmark, CapitalFractionBenchmarkError,
    BENCHMARK_CAPITAL_FRACTION_POLICY_ID as CF_ID,
};
use mqk_backtest::{
    evaluate_scan_candidate, evaluate_scan_candidate_with_emission, evaluate_scan_review_decision,
    execute_strategy_scan_review, write_scan_artifacts, BacktestBar, BacktestConfig,
    BacktestEngine, ReviewRunRequest, ScanBenchmarkPolicy, ScanCapitalFractionBenchmarkEvidence,
    ScanManifest, ScanRunOutput, ScanSummary, SizingPolicy, StrategyScanCandidate,
    StrategyScanPolicy, StrategyScanReasonCode, StrategyScanReviewPolicy, StrategyScanReviewState,
    StrategyScanTruthState,
};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy};

const DAY: i64 = 86_400;
const STRATEGY: &str = "absolute_momentum_252";
const V2_ID: &str = "capital_matched_exact_target_buy_hold_v1";

fn instance() -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(STRATEGY).unwrap()
}

fn bars_from_closes(closes: &[i64]) -> Vec<BacktestBar> {
    closes
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            BacktestBar::new(
                "SPY",
                DAY * (i as i64 + 1),
                c,
                c + 1_000_000,
                c - 1_000_000,
                c,
                1_000,
            )
        })
        .collect()
}

fn rising(n: usize) -> Vec<BacktestBar> {
    let closes: Vec<i64> = (0..n as i64).map(|i| 100_000_000 + i * 50_000).collect();
    bars_from_closes(&closes)
}

fn falling(n: usize) -> Vec<BacktestBar> {
    let closes: Vec<i64> = (0..n as i64).map(|i| 200_000_000 - i * 50_000).collect();
    bars_from_closes(&closes)
}

fn cf_cfg(bps: i64) -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c.sizing_policy = SizingPolicy::capital_fraction_v1(bps).unwrap();
    c
}

fn cf_policy(bps: i64) -> StrategyScanPolicy {
    StrategyScanPolicy {
        base_config: cf_cfg(bps),
        benchmark_policy: ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
        ..StrategyScanPolicy::default()
    }
}

fn cf_review_policy() -> StrategyScanReviewPolicy {
    StrategyScanReviewPolicy {
        benchmark_policy: ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
        ..StrategyScanReviewPolicy::default()
    }
}

fn v2_review_policy() -> StrategyScanReviewPolicy {
    StrategyScanReviewPolicy {
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanReviewPolicy::default()
    }
}

fn cf_candidate(bars: &[BacktestBar], bps: i64) -> StrategyScanCandidate {
    evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(bars),
        &cf_policy(bps),
    )
}

fn v2_candidate(bars: &[BacktestBar]) -> StrategyScanCandidate {
    evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(instance()),
        Some(bars),
        &StrategyScanPolicy {
            benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
            ..StrategyScanPolicy::default()
        },
    )
}

fn legacy_candidate(bars: &[BacktestBar]) -> StrategyScanCandidate {
    evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(bars),
        &StrategyScanPolicy::default(),
    )
}

fn cf_ev(c: &StrategyScanCandidate) -> &ScanCapitalFractionBenchmarkEvidence {
    c.metrics
        .benchmark_capital_fraction
        .as_ref()
        .expect("capital-fraction evidence")
}

// ---------------------------------------------------------------- positives

#[test]
fn benchmark_quantity_is_the_candidates_exact_first_resolved_entry() {
    let bars = rising(320);
    let c = cf_candidate(&bars, 2_500);
    assert_eq!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    let ev = cf_ev(&c);
    assert_eq!(ev.policy_id, CF_ID);

    let mut e = BacktestEngine::new(cf_cfg(2_500));
    e.add_strategy(instance()).unwrap();
    let direct = e.run(&bars).unwrap();
    let first = &direct.sizing_provenance.entries[0];
    assert_eq!(
        ev.benchmark_target_qty_micros,
        first.resolved_target_qty_micros
    );
    assert_eq!(
        ev.candidate_target_qty_micros,
        first.resolved_target_qty_micros
    );
    assert_eq!(ev.reference_bar_end_ts, first.reference_bar_end_ts);
    assert_eq!(
        ev.reference_price_micros,
        first.causal_reference_price_micros
    );
    assert!(ev.benchmark_target_qty_micros > 1_000_000, "never +1 share");
    assert_eq!(ev.candidate_run_id, direct.run_id.to_string());
    ev.verify_internal().expect("genuine evidence verifies");

    assert_eq!(c.metrics.alpha_pct, Some(ev.alpha_pct));
    assert_eq!(
        c.metrics.benchmark_return_pct,
        Some(ev.benchmark_account_return_pct)
    );
    assert_eq!(c.score, c.metrics.alpha_pct);
    assert!(c.metrics.benchmark_v2.is_none());
}

#[test]
fn cost_basis_and_capital_are_matched_and_runs_are_distinct() {
    let bars = rising(320);
    let c = cf_candidate(&bars, 2_500);
    let ev = cf_ev(&c);
    assert_eq!(
        ev.candidate_initial_cash_micros,
        ev.benchmark_initial_cash_micros
    );
    assert_eq!(
        ev.initial_allocated_capital_micros,
        ev.benchmark_initial_cash_micros
    );
    assert_eq!(
        ev.candidate_execution_model_id,
        ev.benchmark_execution_model_id
    );
    assert_eq!(ev.candidate_cost_basis_config_id, ev.benchmark_config_id);
    assert_ne!(ev.candidate_config_id, ev.benchmark_config_id);
    assert_ne!(ev.candidate_run_id, ev.benchmark_run_id);
    assert_eq!(
        ev.position_budget_micros,
        ev.initial_allocated_capital_micros * 2_500 / 10_000
    );
}

#[test]
fn causality_future_bars_do_not_change_entry_quantity_or_reference() {
    let bars = rising(320);
    let a = cf_candidate(&bars, 2_500);
    let ea = cf_ev(&a).clone();
    let entry_idx = bars
        .iter()
        .position(|b| b.end_ts == ea.reference_bar_end_ts)
        .unwrap();
    assert_eq!(ea.reference_price_micros, bars[entry_idx].close_micros);
    assert_eq!(ea.benchmark_entry_bar_index, entry_idx);

    let mut altered = bars.clone();
    for b in altered.iter_mut().skip(entry_idx + 3) {
        b.close_micros = 400_000_000;
        b.open_micros = 400_000_000;
        b.high_micros = 401_000_000;
        b.low_micros = 399_000_000;
    }
    let b = cf_candidate(&altered, 2_500);
    let eb = cf_ev(&b);
    assert_eq!(ea.reference_bar_end_ts, eb.reference_bar_end_ts);
    assert_eq!(ea.reference_price_micros, eb.reference_price_micros);
    assert_eq!(
        ea.benchmark_target_qty_micros,
        eb.benchmark_target_qty_micros
    );
    assert_ne!(
        ea.benchmark_account_return_pct,
        eb.benchmark_account_return_pct
    );
}

#[test]
fn different_fraction_changes_quantity_and_candidate_identity() {
    let bars = rising(320);
    let a = cf_candidate(&bars, 2_500);
    let b = cf_candidate(&bars, 5_000);
    assert_ne!(cf_ev(&a).candidate_config_id, cf_ev(&b).candidate_config_id);
    assert_ne!(
        cf_ev(&a).benchmark_target_qty_micros,
        cf_ev(&b).benchmark_target_qty_micros
    );
}

#[test]
fn never_entering_candidate_has_no_benchmark_and_no_fallback() {
    let flat = cf_candidate(&falling(320), 2_500);
    assert_eq!(
        flat.reason_code,
        StrategyScanReasonCode::BenchmarkUnavailable
    );
    assert_ne!(flat.truth_state, StrategyScanTruthState::CandidateRanked);
    assert!(flat.metrics.benchmark_capital_fraction.is_none());
    assert!(flat.metrics.benchmark_v2.is_none());
    assert!(flat.metrics.alpha_pct.is_none());
    let d = evaluate_scan_review_decision(&flat, &cf_review_policy());
    assert_eq!(d.review_state, StrategyScanReviewState::Blocked);
}

#[test]
fn insufficient_budget_candidate_is_unavailable_never_one_share() {
    let c = cf_candidate(&rising(320), 1);
    assert_ne!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    assert!(c.metrics.benchmark_capital_fraction.is_none());
    assert!(c.metrics.alpha_pct.is_none());
}

// ------------------------------------------- independent economic oracle

struct OracleBuyHold {
    target: mqk_execution::QtyMicros,
    from_idx: usize,
    seen: usize,
}

impl Strategy for OracleBuyHold {
    fn spec(&self) -> mqk_strategy::StrategySpec {
        mqk_strategy::StrategySpec::new("oracle_buy_hold", DAY)
    }
    fn required_history_bars(&self) -> usize {
        0
    }
    fn on_bar(&mut self, _ctx: &mqk_strategy::StrategyContext) -> mqk_execution::StrategyOutput {
        let i = self.seen;
        self.seen += 1;
        let q = if i >= self.from_idx {
            self.target
        } else {
            mqk_execution::QtyMicros::ZERO
        };
        mqk_execution::StrategyOutput::new(vec![mqk_execution::TargetPosition::new("SPY", q)])
    }
}

fn oracle_return_pct(
    cfg: &BacktestConfig,
    bars: &[BacktestBar],
    q_micros: i64,
    from: usize,
) -> f64 {
    let mut c = cfg.clone();
    c.sizing_policy = SizingPolicy::FixedQuantityV1;
    let mut e = BacktestEngine::new(c);
    e.add_strategy(Box::new(OracleBuyHold {
        target: mqk_execution::QtyMicros::new(q_micros),
        from_idx: from,
        seen: 0,
    }))
    .unwrap();
    let r = e.run(bars).unwrap();
    let end = r.equity_curve.last().unwrap().1;
    (end - cfg.initial_cash_micros) as f64 / cfg.initial_cash_micros as f64 * 100.0
}

fn reentry_bars() -> Vec<BacktestBar> {
    let mut closes: Vec<i64> = (0..300).map(|i| 100_000_000 + i * 10_000).collect();
    closes.extend((0..100).map(|i| 103_000_000 - i * 830_000));
    closes.extend(std::iter::repeat(20_000_000).take(300));
    closes.extend((1..40).map(|i| 20_000_000 + i * 200_000));
    bars_from_closes(&closes)
}

#[test]
fn benchmark_return_equals_an_independent_passive_buy_hold_of_the_first_entry() {
    let bars = rising(320);
    let c = cf_candidate(&bars, 2_500);
    assert_eq!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    let ev = cf_ev(&c);
    let oracle = oracle_return_pct(
        &cf_cfg(2_500),
        &bars,
        ev.benchmark_target_qty_micros,
        ev.benchmark_entry_bar_index,
    );
    assert!(oracle != 0.0, "oracle must be a live position");
    assert_eq!(ev.benchmark_account_return_pct, oracle);
    let cand = c.metrics.total_return_pct.expect("candidate return");
    assert!((ev.alpha_pct - (cand - oracle)).abs() < 1e-9);
}

#[test]
fn benchmark_uses_the_first_entry_when_the_candidate_re_enters_at_a_different_size() {
    let bars = reentry_bars();
    let cfg = cf_cfg(2_500);
    let mut e = BacktestEngine::new(cfg.clone());
    e.add_strategy(instance()).unwrap();
    let direct = e.run(&bars).unwrap();
    let entries = &direct.sizing_provenance.entries;
    assert!(
        entries.len() >= 2,
        "fixture must re-enter: {}",
        entries.len()
    );
    assert_ne!(
        entries[0].resolved_target_qty_micros, entries[1].resolved_target_qty_micros,
        "fixture re-entry must resolve a different size"
    );

    let c = cf_candidate(&bars, 2_500);
    assert_eq!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    let ev = cf_ev(&c);
    assert_eq!(
        ev.benchmark_target_qty_micros,
        entries[0].resolved_target_qty_micros
    );
    assert_eq!(ev.reference_bar_end_ts, entries[0].reference_bar_end_ts);
}

// ------------------------------------------------- V2 / legacy untouched

#[test]
fn benchmark_v2_and_legacy_candidates_are_unchanged_and_carry_no_cf_field() {
    let bars = rising(320);
    let v2 = v2_candidate(&bars);
    assert_eq!(v2.metrics.benchmark_v2.as_ref().unwrap().policy_id, V2_ID);
    assert!(v2.metrics.benchmark_capital_fraction.is_none());
    assert!(!serde_json::to_string(&v2)
        .unwrap()
        .contains("benchmark_capital_fraction"));
    assert!(!serde_json::to_string(&legacy_candidate(&bars))
        .unwrap()
        .contains("benchmark_capital_fraction"));
    assert!(serde_json::to_string(&cf_candidate(&bars, 2_500))
        .unwrap()
        .contains("benchmark_capital_fraction"));
}

#[test]
fn scan_policy_pairing_is_enforced_in_the_scanner_both_ways() {
    let bars = rising(320);
    let c = evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(&bars),
        &StrategyScanPolicy {
            base_config: cf_cfg(2_500),
            ..StrategyScanPolicy::default()
        },
    );
    assert_ne!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    assert!(c.metrics.benchmark_capital_fraction.is_none());

    let c = evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(instance()),
        Some(&bars),
        &StrategyScanPolicy {
            base_config: cf_cfg(2_500),
            benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
            ..StrategyScanPolicy::default()
        },
    );
    assert_ne!(c.truth_state, StrategyScanTruthState::CandidateRanked);

    let c = evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(&bars),
        &StrategyScanPolicy {
            benchmark_policy: ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1,
            ..StrategyScanPolicy::default()
        },
    );
    assert_ne!(c.truth_state, StrategyScanTruthState::CandidateRanked);
    assert!(c.metrics.benchmark_capital_fraction.is_none());
}

// ------------------------------------------------------------ direct fn

#[test]
fn direct_function_refuses_non_cf_config_wrong_bars_and_empty_provenance() {
    let bars = rising(320);
    let cfg = cf_cfg(2_500);
    let mut e = BacktestEngine::new(cfg.clone());
    e.add_strategy(instance()).unwrap();
    let report = e.run(&bars).unwrap();
    let section =
        compute_capital_fraction_benchmark(&report, &bars, &cfg, DAY, 7.5).expect("control");
    assert_eq!(section.alpha_pct, 7.5 - section.account_return_pct);

    let mut legacy = cfg.clone();
    legacy.sizing_policy = SizingPolicy::FixedQuantityV1;
    assert!(matches!(
        compute_capital_fraction_benchmark(&report, &bars, &legacy, DAY, 1.0),
        Err(CapitalFractionBenchmarkError::CandidateNotCapitalFraction)
    ));

    let mut altered = bars.clone();
    let idx = bars
        .iter()
        .position(|b| b.end_ts == report.sizing_provenance.entries[0].reference_bar_end_ts)
        .unwrap();
    altered[idx].close_micros += 1_000;
    assert!(matches!(
        compute_capital_fraction_benchmark(&report, &altered, &cfg, DAY, 1.0),
        Err(CapitalFractionBenchmarkError::ProvenanceMismatch(_))
    ));

    let mut no_entries = report.clone();
    no_entries.sizing_provenance.entries.clear();
    assert!(matches!(
        compute_capital_fraction_benchmark(&no_entries, &bars, &cfg, DAY, 1.0),
        Err(CapitalFractionBenchmarkError::CandidateNeverEntered)
    ));

    let mut other_fraction = cfg.clone();
    other_fraction.sizing_policy = SizingPolicy::capital_fraction_v1(5_000).unwrap();
    assert!(matches!(
        compute_capital_fraction_benchmark(&report, &bars, &other_fraction, DAY, 1.0),
        Err(CapitalFractionBenchmarkError::ProvenanceMismatch(_))
    ));

    let mut cash = cfg.clone();
    cash.initial_cash_micros += 1_000_000;
    assert!(matches!(
        compute_capital_fraction_benchmark(&report, &bars, &cash, DAY, 1.0),
        Err(CapitalFractionBenchmarkError::ProvenanceMismatch(_))
    ));
}

// ------------------------------------------------------- review binding

#[test]
fn review_of_genuine_cf_candidate_records_the_evidence() {
    let c = cf_candidate(&rising(320), 2_500);
    let d = evaluate_scan_review_decision(&c, &cf_review_policy());
    assert_ne!(d.review_state, StrategyScanReviewState::Blocked, "{d:?}");
    assert_eq!(
        d.benchmark_capital_fraction,
        c.metrics.benchmark_capital_fraction
    );
    assert!(d.benchmark_v2.is_none());
}

#[test]
fn cross_policy_review_substitution_is_blocked_in_every_direction() {
    let bars = rising(320);
    let cf = cf_candidate(&bars, 2_500);
    let v2 = v2_candidate(&bars);
    let legacy = legacy_candidate(&bars);
    let blocked = |c: &StrategyScanCandidate, p: &StrategyScanReviewPolicy| {
        evaluate_scan_review_decision(c, p).review_state == StrategyScanReviewState::Blocked
    };
    assert!(!blocked(&cf, &cf_review_policy()));
    assert!(!blocked(&v2, &v2_review_policy()));
    assert!(!blocked(&legacy, &StrategyScanReviewPolicy::default()));
    assert!(blocked(&cf, &v2_review_policy()));
    assert!(blocked(&cf, &StrategyScanReviewPolicy::default()));
    assert!(blocked(&v2, &cf_review_policy()));
    assert!(blocked(&legacy, &cf_review_policy()));
}

type Tamper = fn(&mut ScanCapitalFractionBenchmarkEvidence);

#[test]
fn tampered_capital_fraction_evidence_is_blocked() {
    let genuine = cf_candidate(&rising(320), 2_500);
    assert_ne!(
        evaluate_scan_review_decision(&genuine, &cf_review_policy()).review_state,
        StrategyScanReviewState::Blocked,
        "control"
    );
    let cases: Vec<(&str, Tamper)> = vec![
        ("policy_id", |e| e.policy_id = V2_ID.to_string()),
        ("sizing_policy_id", |e| {
            e.sizing_policy_id = "fixed_quantity_v1".into()
        }),
        ("symbol", |e| e.symbol = "QQQ".into()),
        ("strategy_id", |e| e.strategy_id = "other".into()),
        ("fraction_zero", |e| e.allocation_fraction_bps = 0),
        ("fraction_over", |e| e.allocation_fraction_bps = 10_001),
        ("fraction_mismatch_budget", |e| {
            e.allocation_fraction_bps += 1
        }),
        ("capital", |e| {
            e.initial_allocated_capital_micros += 1_000_000
        }),
        ("budget", |e| e.position_budget_micros += 1_000_000),
        ("benchmark_cash", |e| e.benchmark_initial_cash_micros += 1),
        ("candidate_cash", |e| e.candidate_initial_cash_micros += 1),
        ("ref_price_zero", |e| e.reference_price_micros = 0),
        ("qty_candidate", |e| {
            e.candidate_target_qty_micros += 1_000_000
        }),
        ("qty_benchmark", |e| {
            e.benchmark_target_qty_micros += 1_000_000
        }),
        ("qty_zero", |e| {
            e.benchmark_target_qty_micros = 0;
            e.candidate_target_qty_micros = 0;
        }),
        ("qty_fractional", |e| {
            e.benchmark_target_qty_micros += 1;
            e.candidate_target_qty_micros += 1;
        }),
        ("qty_over_budget", |e| {
            e.benchmark_target_qty_micros *= 100;
            e.candidate_target_qty_micros *= 100;
        }),
        ("exec_model", |e| {
            e.benchmark_execution_model_id = "other".into()
        }),
        ("cost_basis_config", |e| {
            e.candidate_cost_basis_config_id = e.candidate_config_id.clone()
        }),
        ("same_run", |e| {
            e.benchmark_run_id = e.candidate_run_id.clone()
        }),
        ("alpha", |e| e.alpha_pct += 1.0),
        ("endpoint", |e| e.evaluation_end_ts += DAY),
        ("entry_index", |e| e.benchmark_entry_bar_index = 10_000),
        ("ref_ts_outside", |e| e.reference_bar_end_ts = 1),
    ];
    for (name, t) in cases {
        let mut c = genuine.clone();
        t(c.metrics.benchmark_capital_fraction.as_mut().unwrap());
        let d = evaluate_scan_review_decision(&c, &cf_review_policy());
        assert_eq!(
            d.review_state,
            StrategyScanReviewState::Blocked,
            "tamper `{name}` must block"
        );
    }
    let mut c = genuine.clone();
    c.metrics.alpha_pct = c.metrics.alpha_pct.map(|a| a + 5.0);
    assert_eq!(
        evaluate_scan_review_decision(&c, &cf_review_policy()).review_state,
        StrategyScanReviewState::Blocked
    );
    let mut c = genuine;
    c.metrics.benchmark_capital_fraction = None;
    assert_eq!(
        evaluate_scan_review_decision(&c, &cf_review_policy()).review_state,
        StrategyScanReviewState::Blocked
    );
}

// ------------------------------------------------------ artifact pairing

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp() -> TmpDir {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "mqk_cf_bench_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    TmpDir(p)
}

fn write_scan(
    root: &std::path::Path,
    candidates: Vec<StrategyScanCandidate>,
    policy_id: Option<&str>,
) -> String {
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
        ranked_count: candidates.len(),
        skipped_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
        benchmark_policy_id: policy_id.map(str::to_string),
    };
    let summary = ScanSummary {
        scan_id: "scan-fixture".to_string(),
        universe_count: 1,
        ranked_count: candidates.len(),
        skipped_count: 0,
        top_ranked: Vec::new(),
        top_skip_reasons: Vec::new(),
    };
    let out = ScanRunOutput {
        scan_id: uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            policy_id.unwrap_or("legacy").as_bytes(),
        ),
        manifest,
        candidates,
        summary,
    };
    write_scan_artifacts(&root.join("scan"), &out)
        .unwrap()
        .display()
        .to_string()
}

fn review_req(scan_dir: &str, policy: StrategyScanReviewPolicy) -> ReviewRunRequest {
    ReviewRunRequest {
        artifact_dir: scan_dir.to_string(),
        top: 5,
        policy,
        git_hash: "TEST".to_string(),
        created_at_utc: "2026-10-03T00:00:00Z".to_string(),
    }
}

#[test]
fn review_run_round_trips_cf_evidence_and_refuses_policy_mismatch() {
    let bars = rising(320);
    let d = tmp();
    let cf_scan = write_scan(d.0.as_path(), vec![cf_candidate(&bars, 2_500)], Some(CF_ID));
    let out = execute_strategy_scan_review(&review_req(&cf_scan, cf_review_policy())).unwrap();
    assert_eq!(out.manifest.benchmark_policy_id.as_deref(), Some(CF_ID));
    let ev = out.decisions[0]
        .benchmark_capital_fraction
        .as_ref()
        .unwrap();
    assert_eq!(ev.policy_id, CF_ID);
    ev.verify_internal().unwrap();

    for wrong in [StrategyScanReviewPolicy::default(), v2_review_policy()] {
        let err = execute_strategy_scan_review(&review_req(&cf_scan, wrong)).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }
    let d2 = tmp();
    let legacy_scan = write_scan(d2.0.as_path(), vec![legacy_candidate(&bars)], None);
    assert!(execute_strategy_scan_review(&review_req(&legacy_scan, cf_review_policy())).is_err());
    let d3 = tmp();
    let v2_scan = write_scan(d3.0.as_path(), vec![v2_candidate(&bars)], Some(V2_ID));
    assert!(execute_strategy_scan_review(&review_req(&v2_scan, cf_review_policy())).is_err());
    assert_ne!(
        out.review_id,
        execute_strategy_scan_review(&review_req(
            &legacy_scan,
            StrategyScanReviewPolicy::default()
        ))
        .unwrap()
        .review_id
    );
}
