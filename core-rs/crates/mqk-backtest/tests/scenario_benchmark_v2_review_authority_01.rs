//! Benchmark V2 as the actual scanner/review alpha authority for exact-target
//! native candidates.
//!
//! Every candidate here is produced by the REAL production path
//! (`evaluate_scan_candidate_with_emission` -> `BacktestEngine` ->
//! `emit_native_signal_stream` -> `compute_benchmark_v2`) over real native
//! strategies; negative controls tamper with that genuine evidence one field
//! at a time. No DB, network, or broker access.

use mqk_backtest::{
    evaluate_scan_candidate, evaluate_scan_candidate_with_emission, evaluate_scan_review_decision,
    execute_strategy_scan_review, write_review_artifacts, write_scan_artifacts, BacktestBar,
    BacktestConfig, ReviewRunRequest, ScanBenchmarkPolicy, ScanManifest, ScanRunOutput,
    ScanSummary, StrategyScanCandidate, StrategyScanPolicy, StrategyScanReasonCode,
    StrategyScanReviewPolicy, StrategyScanReviewState, StrategyScanTruthState,
};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{PluginRegistry, Strategy};

const DAY: i64 = 86_400;
const STRATEGY: &str = "absolute_momentum_252";
const V2_ID: &str = "capital_matched_exact_target_buy_hold_v1";

fn instance(name: &str) -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(name).unwrap()
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

fn v2_policy() -> StrategyScanPolicy {
    StrategyScanPolicy {
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanPolicy::default()
    }
}

fn v2_review_policy() -> StrategyScanReviewPolicy {
    StrategyScanReviewPolicy {
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanReviewPolicy::default()
    }
}

fn v2_candidate(bars: &[BacktestBar]) -> StrategyScanCandidate {
    evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance(STRATEGY)),
        Some(instance(STRATEGY)),
        Some(bars),
        &v2_policy(),
    )
}

fn legacy_candidate(bars: &[BacktestBar]) -> StrategyScanCandidate {
    evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance(STRATEGY)),
        Some(bars),
        &StrategyScanPolicy::default(),
    )
}

// ---------------------------------------------------------------- positives

/// REQUIRED 1/5: alpha is Benchmark V2 matched alpha -- not the legacy
/// fully-invested alpha -- and the legacy return survives as information.
#[test]
fn exact_target_candidate_alpha_is_benchmark_v2_alpha_not_legacy() {
    let bars = rising(320);
    let v2 = v2_candidate(&bars);
    let legacy = legacy_candidate(&bars);
    assert_eq!(v2.truth_state, StrategyScanTruthState::CandidateRanked);
    let ev = v2.metrics.benchmark_v2.as_ref().expect("V2 evidence");
    assert_eq!(ev.policy_id, V2_ID);

    // alpha is exactly candidate return minus the capital-matched benchmark.
    assert_eq!(v2.metrics.alpha_pct, Some(ev.alpha_pct));
    assert_eq!(
        v2.metrics.benchmark_return_pct,
        Some(ev.benchmark_account_return_pct)
    );
    assert!(
        (ev.candidate_total_return_pct - ev.benchmark_account_return_pct - ev.alpha_pct).abs()
            < 1e-12
    );
    // score ranks on V2 alpha.
    assert_eq!(v2.score, v2.metrics.alpha_pct);

    // The legacy fully-invested benchmark on a rising series is large; the
    // one-share capital-matched V2 benchmark is tiny. If the V2 path silently
    // reused legacy alpha, these would be equal.
    let legacy_alpha = legacy.metrics.alpha_pct.expect("legacy alpha");
    assert!(
        (legacy_alpha - ev.alpha_pct).abs() > 1.0,
        "V2 alpha {} must differ materially from legacy alpha {legacy_alpha}",
        ev.alpha_pct
    );
    // Legacy return is retained, informational only.
    assert_eq!(
        ev.legacy_buy_and_hold_return_pct,
        legacy.metrics.benchmark_return_pct
    );
    assert_ne!(
        v2.metrics.benchmark_return_pct,
        ev.legacy_buy_and_hold_return_pct
    );
}

/// REQUIRED 2: the alpha gate stays at 0 under the V2 policy.
#[test]
fn min_alpha_pct_stays_zero_under_v2_review_policy() {
    assert_eq!(v2_review_policy().min_alpha_pct, 0.0);
    assert_eq!(StrategyScanReviewPolicy::default().min_alpha_pct, 0.0);
}

/// REQUIRED 3/4/5: one-share candidate vs one-share benchmark, same capital
/// basis, benchmark starts at the candidate's first causally eligible bar.
#[test]
fn one_share_capital_matched_and_eligibility_matched() {
    let bars = rising(320);
    let c = v2_candidate(&bars);
    let ev = c.metrics.benchmark_v2.as_ref().unwrap();
    assert_eq!(ev.candidate_target_qty_micros, 1_000_000);
    assert_eq!(ev.benchmark_target_qty_micros, 1_000_000);
    assert_eq!(
        ev.candidate_initial_cash_micros,
        BacktestConfig::conservative_defaults().initial_cash_micros
    );
    assert_eq!(
        ev.candidate_initial_cash_micros,
        ev.benchmark_initial_cash_micros
    );
    assert_eq!(ev.candidate_config_id, ev.benchmark_config_id);
    assert_eq!(ev.required_history_bars, 253);
    assert_eq!(ev.benchmark_eligibility_bar_index, 252);
    assert_eq!(ev.benchmark_eligibility_decision_ts, bars[252].end_ts);
    assert_eq!(ev.evaluation_end_ts, bars.last().unwrap().end_ts);
    assert_ne!(ev.candidate_run_id, ev.benchmark_run_id);
    ev.verify_internal().expect("genuine evidence verifies");
}

/// A genuine V2 candidate reviews cleanly and the decision row carries the
/// exact evidence (what promotion later binds).
#[test]
fn review_of_genuine_v2_candidate_records_the_evidence() {
    let bars = rising(320);
    let c = v2_candidate(&bars);
    let d = evaluate_scan_review_decision(&c, &v2_review_policy());
    assert_ne!(d.review_state, StrategyScanReviewState::Blocked, "{d:?}");
    assert_eq!(d.benchmark_v2, c.metrics.benchmark_v2);
}

/// REQUIRED 6: the review artifact records the V2 policy at manifest and row
/// level, and a legacy review artifact carries neither.
#[test]
fn review_artifacts_record_the_benchmark_policy() {
    let bars = rising(320);
    let cand = v2_candidate(&bars);
    let dir = tmp();
    let scan_dir = write_scan(dir.path(), vec![cand], Some(V2_ID));
    let out = execute_strategy_scan_review(&review_req(&scan_dir, v2_review_policy())).unwrap();
    assert_eq!(out.manifest.benchmark_policy_id.as_deref(), Some(V2_ID));
    assert_eq!(out.manifest.policy_min_alpha_pct, 0.0);
    assert_eq!(
        out.decisions[0].benchmark_v2.as_ref().unwrap().policy_id,
        V2_ID
    );
    let run_dir = write_review_artifacts(&dir.path().join("rev"), &out).unwrap();
    let json = std::fs::read_to_string(run_dir.join("review_decisions.json")).unwrap();
    assert!(json.contains(V2_ID));

    // legacy scan + legacy review: no V2 vocabulary anywhere, historical bytes.
    let legacy_dir = tmp();
    let lscan = write_scan(legacy_dir.path(), vec![legacy_candidate(&bars)], None);
    let lout =
        execute_strategy_scan_review(&review_req(&lscan, StrategyScanReviewPolicy::default()))
            .unwrap();
    assert!(lout.manifest.benchmark_policy_id.is_none());
    assert!(lout.decisions[0].benchmark_v2.is_none());
    let lrun = write_review_artifacts(&legacy_dir.path().join("rev"), &lout).unwrap();
    for f in ["manifest.json", "review_decisions.json"] {
        let text = std::fs::read_to_string(lrun.join(f)).unwrap();
        assert!(
            !text.contains("benchmark"),
            "legacy artifact {f} must not mention benchmark policy"
        );
    }
    assert_ne!(out.review_id, lout.review_id);
}

// ------------------------------------------------- scan/review policy pairing

/// A legacy scan can never be reviewed as V2 authority, and a V2 scan is not
/// reviewable under the legacy contract.
#[test]
fn review_refuses_a_policy_mismatch_with_the_scan_in_both_directions() {
    let bars = rising(320);
    let d1 = tmp();
    let legacy_scan = write_scan(d1.path(), vec![legacy_candidate(&bars)], None);
    let err = execute_strategy_scan_review(&review_req(&legacy_scan, v2_review_policy()))
        .expect_err("legacy scan reviewed as V2 must be refused");
    assert!(err.contains("does not match"), "{err}");

    let d2 = tmp();
    let v2_scan = write_scan(d2.path(), vec![v2_candidate(&bars)], Some(V2_ID));
    let err =
        execute_strategy_scan_review(&review_req(&v2_scan, StrategyScanReviewPolicy::default()))
            .expect_err("V2 scan reviewed as legacy must be refused");
    assert!(err.contains("does not match"), "{err}");

    let d3 = tmp();
    let bogus = write_scan(
        d3.path(),
        vec![v2_candidate(&bars)],
        Some("some_other_policy"),
    );
    let err = execute_strategy_scan_review(&review_req(&bogus, v2_review_policy()))
        .expect_err("unknown manifest policy id must be refused");
    assert!(err.contains("unrecognized benchmark_policy_id"), "{err}");
}

// ---------------------------------------------------- fail-closed production

/// A V2 scan that cannot produce a benchmark is Blocked, never legacy-judged:
/// (a) no emission instance, (b) a strategy that never takes a position.
#[test]
fn v2_scan_without_a_computable_benchmark_fails_closed() {
    let bars = rising(320);
    let no_emission = evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance(STRATEGY)),
        None,
        Some(&bars),
        &v2_policy(),
    );
    assert_eq!(
        no_emission.truth_state,
        StrategyScanTruthState::MetricsUnavailable
    );
    assert_eq!(
        no_emission.reason_code,
        StrategyScanReasonCode::BenchmarkUnavailable
    );
    assert!(no_emission.metrics.alpha_pct.is_none());
    assert!(no_emission.score.is_none());

    // The legacy entry point under a V2 policy has no emission instance.
    let via_legacy_fn = evaluate_scan_candidate(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance(STRATEGY)),
        Some(&bars),
        &v2_policy(),
    );
    assert_eq!(
        via_legacy_fn.reason_code,
        StrategyScanReasonCode::BenchmarkUnavailable
    );

    // Falling market: absolute momentum never goes long -> no positive target.
    let flat = v2_candidate(&falling(320));
    assert_eq!(
        flat.reason_code,
        StrategyScanReasonCode::BenchmarkUnavailable
    );
    let d = evaluate_scan_review_decision(&flat, &v2_review_policy());
    assert_eq!(d.review_state, StrategyScanReviewState::Blocked);
    assert_eq!(d.reason_codes, vec!["not_candidate_ranked".to_string()]);
}

// ------------------------------------------------------ review negatives

type Tamper = fn(&mut StrategyScanCandidate);

fn ev(c: &mut StrategyScanCandidate) -> &mut mqk_backtest::ScanBenchmarkV2Evidence {
    c.metrics.benchmark_v2.as_mut().unwrap()
}

/// REQUIRED NEGATIVES 1-12: each tamper of genuine evidence must Block with
/// the stated reason; none may reach any alpha/return gate.
#[test]
fn tampered_or_mismatched_benchmark_evidence_is_blocked() {
    let bars = rising(320);
    let genuine = v2_candidate(&bars);
    assert_ne!(
        evaluate_scan_review_decision(&genuine, &v2_review_policy()).review_state,
        StrategyScanReviewState::Blocked,
        "control: the untampered candidate must pass the binding"
    );

    let cases: Vec<(&str, &str, Tamper)> = vec![
        ("1 missing artifact", "missing_benchmark_v2", |c| {
            c.metrics.benchmark_v2 = None
        }),
        ("3 wrong policy id", "benchmark_policy_mismatch", |c| {
            ev(c).policy_id = "buy_and_hold_legacy".to_string()
        }),
        ("4 wrong quantity", "benchmark_binding_mismatch", |c| {
            ev(c).benchmark_target_qty_micros += 1_000_000
        }),
        ("5 wrong capital", "benchmark_binding_mismatch", |c| {
            ev(c).benchmark_initial_cash_micros = 50_000_000_000
        }),
        ("6a wrong strategy id", "benchmark_binding_mismatch", |c| {
            ev(c).strategy_id = "dual_sma_50_200_trend".to_string()
        }),
        (
            "6b blank strategy fingerprint",
            "benchmark_binding_mismatch",
            |c| ev(c).strategy_semantic_fingerprint.clear(),
        ),
        ("7 wrong symbol", "benchmark_binding_mismatch", |c| {
            ev(c).symbol = "QQQ".to_string()
        }),
        ("8 wrong timeframe", "benchmark_binding_mismatch", |c| {
            ev(c).timeframe = "1H".to_string()
        }),
        (
            "9 candidate run id == benchmark run id",
            "benchmark_binding_mismatch",
            |c| {
                let b = ev(c).benchmark_run_id.clone();
                ev(c).candidate_run_id = b;
            },
        ),
        (
            "10a wrong data endpoint",
            "benchmark_binding_mismatch",
            |c| ev(c).evaluation_end_ts += DAY,
        ),
        (
            "10b blank data identity",
            "benchmark_binding_mismatch",
            |c| ev(c).input_data_hash.clear(),
        ),
        (
            "11 eligibility shifted to first bar",
            "benchmark_binding_mismatch",
            |c| ev(c).benchmark_eligibility_bar_index = 0,
        ),
        (
            "12a cost/capital config differs",
            "benchmark_binding_mismatch",
            |c| ev(c).benchmark_config_id = "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "12b execution model differs",
            "benchmark_binding_mismatch",
            |c| ev(c).benchmark_execution_model_id = "frictionless".to_string(),
        ),
        (
            "2 legacy alpha substituted into metrics",
            "benchmark_binding_mismatch",
            |c| {
                let legacy = c
                    .metrics
                    .benchmark_v2
                    .as_ref()
                    .unwrap()
                    .legacy_buy_and_hold_return_pct
                    .unwrap();
                c.metrics.benchmark_return_pct = Some(legacy);
                c.metrics.alpha_pct = Some(c.metrics.total_return_pct.unwrap() - legacy);
            },
        ),
        (
            "alpha not return minus benchmark",
            "benchmark_binding_mismatch",
            |c| {
                ev(c).alpha_pct += 5.0;
                let a = ev(c).alpha_pct;
                c.metrics.alpha_pct = Some(a);
            },
        ),
    ];
    for (name, expected, tamper) in cases {
        let mut c = genuine.clone();
        tamper(&mut c);
        let d = evaluate_scan_review_decision(&c, &v2_review_policy());
        assert_eq!(
            d.review_state,
            StrategyScanReviewState::Blocked,
            "{name}: {d:?}"
        );
        assert_eq!(d.reason_codes, vec![expected.to_string()], "{name}");
        assert!(
            d.benchmark_v2.is_none(),
            "{name}: blocked rows carry no evidence"
        );
    }

    // A legacy candidate (no V2 evidence) cannot satisfy the V2 review policy,
    // however good its legacy alpha looks.
    let legacy = legacy_candidate(&bars);
    let d = evaluate_scan_review_decision(&legacy, &v2_review_policy());
    assert_eq!(d.review_state, StrategyScanReviewState::Blocked);
    assert_eq!(d.reason_codes, vec!["missing_benchmark_v2".to_string()]);
}

/// The legacy review path is unchanged: a legacy candidate still reviews by
/// legacy alpha under the default policy.
#[test]
fn legacy_review_path_is_unchanged() {
    let bars = rising(320);
    let legacy = legacy_candidate(&bars);
    let d = evaluate_scan_review_decision(&legacy, &StrategyScanReviewPolicy::default());
    assert_ne!(
        d.reason_codes,
        vec!["missing_benchmark_v2".to_string()],
        "legacy path must not demand V2 evidence"
    );
    assert!(d.benchmark_v2.is_none());
}

// ------------------------------------------------------------ test helpers

struct TmpDir(std::path::PathBuf);

impl TmpDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp() -> TmpDir {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "mqk_bv2_review_{}_{}",
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
