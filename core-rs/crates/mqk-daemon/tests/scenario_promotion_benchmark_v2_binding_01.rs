//! Promotion binding for corrected native (exact-target) evidence: the review
//! that produced the `paper_candidate` row must have judged alpha against
//! Benchmark V2 for exactly the candidate being promoted.
//!
//! The review artifacts here come from the real production path (real native
//! strategy -> scanner V2 evidence -> `execute_strategy_scan_review` ->
//! `write_review_artifacts`) and are read back through the real
//! `validate_paper_candidate_evidence`. Only the final `review_state` of the
//! fixture row is forced to `paper_candidate` (the one-share fixture series
//! has no completed round trip, so honest classification is `watchlist`);
//! the binding under test never looks at the classification.

use mqk_backtest::{
    evaluate_scan_candidate, evaluate_scan_candidate_with_emission, execute_strategy_scan_review,
    write_review_artifacts, write_scan_artifacts, BacktestBar, BacktestEngine, BacktestReport,
    ReviewRunRequest, ScanBenchmarkPolicy, ScanManifest, ScanRunOutput, ScanSummary,
    StrategyScanCandidate, StrategyScanPolicy, StrategyScanReviewPolicy, StrategyScanReviewState,
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
const CAPITAL: i64 = 100_000_000_000;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn instance() -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(STRATEGY).unwrap()
}

fn bars() -> Vec<BacktestBar> {
    (0..320i64)
        .map(|i| {
            let c = 100_000_000 + i * 50_000;
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

fn v2_candidate(bars: &[BacktestBar]) -> StrategyScanCandidate {
    let policy = StrategyScanPolicy {
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanPolicy::default()
    };
    evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(instance()),
        Some(bars),
        &policy,
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

struct Fx {
    _dir: tempfile::TempDir,
    review_dir: String,
    st: AppState,
}

/// Run the real scan-artifact -> review -> review-artifact chain for one
/// candidate and return a state configured to read it.
fn fixture(candidate: StrategyScanCandidate, benchmark: ScanBenchmarkPolicy) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let policy_id = benchmark.policy_id().map(str::to_string);
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
        benchmark_policy_id: policy_id.clone(),
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
        scan_id: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"fixture"),
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
    out.decisions[0].review_state = StrategyScanReviewState::PaperCandidate;
    let review_dir = write_review_artifacts(&dir.path().join("reviews"), &out).unwrap();

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", dir.path());
    let st = AppState::new_with_operator_auth(OperatorAuthMode::ExplicitDevNoToken);
    Fx {
        _dir: dir,
        review_dir: review_dir.display().to_string(),
        st,
    }
}

fn validated(fx: &Fx) -> ValidatedEvidence {
    validate_paper_candidate_evidence(&fx.st, &fx.review_dir, STRATEGY, "SPY", DAY)
        .expect("fixture review evidence must validate")
}

struct Expect {
    fingerprint: String,
    capital: i64,
    data_hash: String,
}

fn expect_for(cand: &StrategyScanCandidate) -> Expect {
    let ev = cand.metrics.benchmark_v2.as_ref().expect("V2 evidence");
    Expect {
        fingerprint: ev.strategy_semantic_fingerprint.clone(),
        capital: CAPITAL,
        data_hash: ev.input_data_hash.clone(),
    }
}

/// The canonical Backtest evidence for the fixture candidate: a real engine
/// run under the SAME config the scanned candidate used (the scanner's
/// default base config at the strategy's timeframe, default sizing).
fn canonical_report() -> BacktestReport {
    let mut cfg = StrategyScanPolicy::default().base_config;
    cfg.timeframe_secs = DAY;
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(instance()).unwrap();
    engine.run(&bars()).expect("canonical backtest run")
}

/// The binding exactly as the promotion route calls it; `capital` and `hash`
/// let a case perturb the canonical evidence's capital / data identity.
fn enforce(
    ev: Option<&ValidatedEvidence>,
    fp: Option<&str>,
    strategy: &str,
    symbol: &str,
    tf: i64,
    capital: i64,
    hash: &str,
) -> Result<(), String> {
    let mut report = canonical_report();
    report.input_data_hash = hash.to_string();
    let identity = BacktestEvidenceIdentity::from_report(&report, capital);
    enforce_native_review_benchmark_binding(fp, ev, strategy, symbol, tf, &identity)
}

/// REQUIRED POSITIVE 7: a correctly bound Benchmark V2 review is accepted
/// when every other fixture field is valid.
#[test]
fn correctly_bound_benchmark_v2_review_is_accepted() {
    let cand = v2_candidate(&bars());
    let x = expect_for(&cand);
    let fx = fixture(cand, ScanBenchmarkPolicy::CapitalMatchedExactTargetV2);
    let ev = validated(&fx);
    assert_eq!(
        ev.benchmark_policy_id.as_deref(),
        Some("capital_matched_exact_target_buy_hold_v1")
    );
    enforce(
        Some(&ev),
        Some(&x.fingerprint),
        STRATEGY,
        "SPY",
        DAY,
        x.capital,
        &x.data_hash,
    )
    .expect("a correctly bound V2 review must be accepted");
}

/// Corrected native Research evidence + a legacy-benchmark review is refused,
/// however the legacy row scores.
#[test]
fn corrected_native_evidence_with_a_legacy_review_is_refused() {
    let bars = bars();
    let v2 = v2_candidate(&bars);
    let x = expect_for(&v2);
    let fx = fixture(
        legacy_candidate(&bars),
        ScanBenchmarkPolicy::LegacyFullyInvested,
    );
    let ev = validated(&fx);
    assert!(ev.benchmark_policy_id.is_none() && ev.benchmark_v2.is_none());
    let err = enforce(
        Some(&ev),
        Some(&x.fingerprint),
        STRATEGY,
        "SPY",
        DAY,
        x.capital,
        &x.data_hash,
    )
    .expect_err("legacy review must not authorize corrected native evidence");
    assert!(err.contains("requires a Benchmark V2 review"), "{err}");

    // No review evidence at all is refused too.
    assert!(enforce(
        None,
        Some(&x.fingerprint),
        STRATEGY,
        "SPY",
        DAY,
        x.capital,
        &x.data_hash
    )
    .is_err());

    // Legacy (non-native) Research evidence is outside this binding.
    assert!(enforce(
        Some(&ev),
        None,
        STRATEGY,
        "SPY",
        DAY,
        x.capital,
        &x.data_hash
    )
    .is_ok());
}

/// A genuine V2 review bound to a DIFFERENT strategy / symbol / timeframe /
/// semantic fingerprint / capital / data identity is refused.
#[test]
fn benchmark_v2_review_for_a_different_identity_is_refused() {
    let cand = v2_candidate(&bars());
    let x = expect_for(&cand);
    let fx = fixture(cand, ScanBenchmarkPolicy::CapitalMatchedExactTargetV2);
    let ev = validated(&fx);
    let fp = x.fingerprint.as_str();
    let h = x.data_hash.as_str();
    let other_fp = "f".repeat(64);

    let cases: Vec<(&str, Result<(), String>)> = vec![
        (
            "control",
            enforce(Some(&ev), Some(fp), STRATEGY, "SPY", DAY, x.capital, h),
        ),
        (
            "wrong strategy",
            enforce(
                Some(&ev),
                Some(fp),
                "dual_sma_50_200_trend",
                "SPY",
                DAY,
                x.capital,
                h,
            ),
        ),
        (
            "wrong symbol",
            enforce(Some(&ev), Some(fp), STRATEGY, "QQQ", DAY, x.capital, h),
        ),
        (
            "wrong timeframe",
            enforce(Some(&ev), Some(fp), STRATEGY, "SPY", 3_600, x.capital, h),
        ),
        (
            "wrong fingerprint",
            enforce(
                Some(&ev),
                Some(&other_fp),
                STRATEGY,
                "SPY",
                DAY,
                x.capital,
                h,
            ),
        ),
        (
            "wrong capital",
            enforce(Some(&ev), Some(fp), STRATEGY, "SPY", DAY, 50_000_000_000, h),
        ),
        (
            "wrong data identity",
            enforce(
                Some(&ev),
                Some(fp),
                STRATEGY,
                "SPY",
                DAY,
                x.capital,
                "00000000-0000-0000-0000-000000000000",
            ),
        ),
        (
            "blank data identity",
            enforce(Some(&ev), Some(fp), STRATEGY, "SPY", DAY, x.capital, ""),
        ),
    ];
    for (name, res) in cases {
        if name == "control" {
            assert!(res.is_ok(), "control must pass: {res:?}");
        } else {
            assert!(res.is_err(), "{name} must be refused");
        }
    }
}

/// Tampering with the validated row's benchmark evidence after the fact (as a
/// forged review artifact would) is caught by the binding.
#[test]
fn forged_review_evidence_is_refused() {
    let cand = v2_candidate(&bars());
    let x = expect_for(&cand);
    let fx = fixture(cand, ScanBenchmarkPolicy::CapitalMatchedExactTargetV2);
    let good = validated(&fx);
    let run = |ev: &ValidatedEvidence| {
        enforce(
            Some(ev),
            Some(&x.fingerprint),
            STRATEGY,
            "SPY",
            DAY,
            x.capital,
            &x.data_hash,
        )
    };
    assert!(run(&good).is_ok());

    let mut e = good.clone();
    e.benchmark_policy_id = None;
    assert!(run(&e).is_err(), "manifest not recorded under V2");

    let mut e = good.clone();
    e.benchmark_v2 = None;
    assert!(run(&e).is_err(), "row without V2 evidence");

    let mut e = good.clone();
    e.benchmark_v2.as_mut().unwrap().policy_id = "legacy".to_string();
    assert!(run(&e).is_err(), "wrong policy id in row");

    let mut e = good.clone();
    e.benchmark_v2.as_mut().unwrap().benchmark_target_qty_micros += 1_000_000;
    assert!(
        run(&e).is_err(),
        "benchmark quantity differs from candidate"
    );

    let mut e = good.clone();
    e.benchmark_v2
        .as_mut()
        .unwrap()
        .benchmark_eligibility_bar_index = 0;
    assert!(run(&e).is_err(), "eligibility shifted to first data bar");

    let mut e = good.clone();
    e.benchmark_v2.as_mut().unwrap().benchmark_config_id =
        "00000000-0000-0000-0000-000000000000".to_string();
    assert!(run(&e).is_err(), "benchmark cost/capital config differs");

    let mut e = good.clone();
    e.scanner_score = Some(good.scanner_score.unwrap() + 1.0);
    assert!(run(&e).is_err(), "score is not the V2 alpha");

    let mut e = good;
    e.scanner_score = None;
    assert!(run(&e).is_err(), "missing score");
}

/// Wiring guard: the production promotion route must run the native review
/// binding after the Research gate yields verified evidence and before the
/// canonical `evaluate_promotion` decision. (The route itself is DB-backed;
/// this proves the call cannot be silently removed or reordered.)
#[test]
fn promotion_route_enforces_the_native_review_binding_before_evaluate_promotion() {
    let src = include_str!("../src/routes/strategy_promotions.rs").replace("\r\n", "\n");
    let research = src
        .find("evaluate_research_evidence_gate(")
        .expect("research gate call");
    let enforce = src
        .find("enforce_native_review_benchmark_binding(")
        .expect("native review benchmark binding call");
    let decide = src
        .find("mqk_promotion::evaluate_promotion(")
        .expect("canonical promotion decision");
    assert!(research < enforce && enforce < decide);
    let call = &src[enforce..decide];
    let call: String = call.split_whitespace().collect();
    assert!(call.contains("oos_evidence.native_semantic_fingerprint()"));
    assert!(call.contains("BacktestEvidenceIdentity::from_bundle(&backtest_bundle"));
    assert!(call.contains("evidence.as_ref()"));
}

/// Wiring guard: the production promotion route must refuse a capital-fraction
/// candidate whose stress evidence differs from the stress its REGISTERED
/// Research trial predeclared, using the verified OOS evidence (registry) and
/// the resolved Backtest bundle -- after the Research gate, before the decision.
#[test]
fn promotion_route_enforces_the_registered_stress_contract_before_evaluate_promotion() {
    let src = include_str!("../src/routes/strategy_promotions.rs").replace(
        "
", "
",
    );
    let research = src
        .find("evaluate_research_evidence_gate(")
        .expect("research gate call");
    let stress = src
        .find("verify_registered_stress_contract(")
        .expect("registered stress contract binding call");
    let decide = src
        .find("mqk_promotion::evaluate_promotion(")
        .expect("canonical promotion decision");
    assert!(research < stress && stress < decide);
    let call: String = src[stress..decide].split_whitespace().collect();
    assert!(call.contains("(&backtest_bundle,&oos_evidence)"));
    assert!(
        call.contains("return transition_response"),
        "a refusal must end the request"
    );
}
