//! IR-BV2-01: a Benchmark V2 review row must be bound to the SAME
//! promotion-relevant execution/economic contract as the canonical Backtest
//! evidence Promotion consumes -- not just to its own passive benchmark.
//!
//! Both sides here come from the real production path: the review row from
//! the real scanner V2 evaluation -> `execute_strategy_scan_review` ->
//! `write_review_artifacts` -> `validate_paper_candidate_evidence`, and the
//! canonical Backtest evidence from a real `BacktestEngine` run reported
//! through the same `BacktestReport` that `resolve_backtest_evidence` loads.

use mqk_backtest::{
    evaluate_scan_candidate_with_emission, execute_strategy_scan_review, write_review_artifacts,
    write_scan_artifacts, BacktestBar, BacktestConfig, BacktestEngine, BacktestInstrumentEconomics,
    BacktestReport, QuantitySemanticsId, ReviewRunRequest, ScanBenchmarkPolicy, ScanManifest,
    ScanRunOutput, ScanSummary, StrategyScanCandidate, StrategyScanPolicy,
    StrategyScanReviewPolicy, StrategyScanReviewState,
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

/// The canonical promotion Backtest configuration the way `backtest csv`
/// builds it for daily bars (integrity ON with a daily stale threshold and a
/// gap tolerance) -- deliberately NOT the scanner's legacy default
/// (integrity forced off, stale 120, gap 0).
fn canonical_cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = DAY;
    cfg.integrity_enabled = true;
    cfg.integrity_stale_threshold_ticks = 259_200;
    cfg.integrity_gap_tolerance_bars = 3;
    cfg
}

fn scan_candidate(cfg: &BacktestConfig, bars: &[BacktestBar]) -> StrategyScanCandidate {
    let policy = StrategyScanPolicy {
        base_config: cfg.clone(),
        benchmark_policy: ScanBenchmarkPolicy::CapitalMatchedExactTargetV2,
        ..StrategyScanPolicy::default()
    };
    let cand = evaluate_scan_candidate_with_emission(
        "SPY",
        "1D",
        STRATEGY,
        Some(DAY),
        Some(instance()),
        Some(instance()),
        Some(bars),
        &policy,
    );
    assert!(
        cand.metrics.benchmark_v2.is_some(),
        "fixture candidate must carry V2 evidence: {cand:?}"
    );
    cand
}

/// A real canonical Backtest run (what Promotion's evidence resolver loads).
fn backtest_report(
    cfg: &BacktestConfig,
    economics: Option<BacktestInstrumentEconomics>,
    bars: &[BacktestBar],
) -> BacktestReport {
    let mut engine = BacktestEngine::new(cfg.clone());
    if let Some(e) = economics {
        engine = engine.with_economics(e);
    }
    engine.add_strategy(instance()).unwrap();
    engine.run(bars).expect("canonical backtest run")
}

struct Fx {
    _dir: tempfile::TempDir,
    st: AppState,
    review_dir: String,
}

fn fixture(candidate: StrategyScanCandidate) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let benchmark = ScanBenchmarkPolicy::CapitalMatchedExactTargetV2;
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
    // The one-share fixture has no completed round trip, so honest
    // classification is not paper_candidate; the binding never looks at it.
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

/// The production binding, fed exactly what the promotion route feeds it:
/// the native fingerprint, the validated review row, and the canonical
/// Backtest evidence's identity.
fn promote_binding(
    ev: &ValidatedEvidence,
    report: &BacktestReport,
    capital: i64,
) -> Result<(), String> {
    let fp = ev
        .benchmark_v2
        .as_ref()
        .unwrap()
        .strategy_semantic_fingerprint
        .clone();
    let identity = BacktestEvidenceIdentity::from_report(report, capital);
    enforce_native_review_benchmark_binding(Some(&fp), Some(ev), STRATEGY, "SPY", DAY, &identity)
}

const CAPITAL: i64 = 100_000_000_000;

/// RED -> GREEN controller. The review row was judged under config A (the
/// scanner-default integrity posture); the canonical Backtest evidence was run
/// under config B. Every field the old binding checked (strategy, symbol,
/// timeframe, fingerprint, capital, bars) matches -- only outcome-bearing
/// integrity configuration differs. Promotion must refuse.
#[test]
fn review_scored_under_a_different_config_than_the_canonical_backtest_is_refused() {
    let bars = bars();
    let scanner_default_cand = {
        // Config A: the scanner's own default (integrity off, stale 120, gap 0).
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
            Some(&bars),
            &policy,
        )
    };
    let fx = fixture(scanner_default_cand);
    let ev = validated(&fx);
    let report_b = backtest_report(&canonical_cfg(), None, &bars);

    // Preconditions: everything the old binding checked agrees.
    let b = ev.benchmark_v2.as_ref().unwrap();
    assert_eq!(b.input_data_hash, report_b.input_data_hash);
    assert_eq!(b.candidate_execution_model_id, report_b.execution_model_id);
    assert_eq!(
        b.strategy_semantic_fingerprint,
        report_b.strategy_semantic_fingerprint
    );
    assert_ne!(b.candidate_config_id, report_b.config_id.to_string());

    let res = promote_binding(&ev, &report_b, CAPITAL);
    assert!(
        res.is_err(),
        "a review scored under a different execution config must not authorize \
         promotion on the canonical Backtest evidence: {res:?}"
    );
}

/// Positive: the exact same canonical configuration on both sides is accepted.
#[test]
fn review_scored_under_the_canonical_backtest_config_is_accepted() {
    let bars = bars();
    let cfg = canonical_cfg();
    let fx = fixture(scan_candidate(&cfg, &bars));
    let ev = validated(&fx);
    let report = backtest_report(&cfg, None, &bars);
    let b = ev.benchmark_v2.as_ref().unwrap();
    assert_eq!(b.candidate_config_id, report.config_id.to_string());
    assert_eq!(b.candidate_run_id, report.run_id.to_string());
    promote_binding(&ev, &report, CAPITAL).expect("matching config + economics must be accepted");
}

type CfgMutation = fn(&mut BacktestConfig);

/// Every outcome-bearing `BacktestConfig` field: the review row is scored
/// under the canonical config; the canonical Backtest evidence is a REAL run
/// under the same config with exactly one field changed. Each must be refused.
#[test]
fn each_outcome_bearing_config_field_mismatch_is_refused() {
    let bars = bars();
    let cfg = canonical_cfg();
    let fx = fixture(scan_candidate(&cfg, &bars));
    let ev = validated(&fx);
    let control = backtest_report(&cfg, None, &bars);
    promote_binding(&ev, &control, CAPITAL).expect("control must pass");

    let cases: Vec<(&str, CfgMutation)> = vec![
        ("commission per share", |c| {
            c.commission.per_share_micros += 1_000
        }),
        ("commission bps", |c| c.commission.bps_of_notional = 3),
        ("base slippage", |c| c.stress.slippage_bps += 1),
        ("volatility component", |c| {
            c.stress.volatility_mult_bps += 100
        }),
        ("participation impact", |c| {
            c.stress.participation_impact_bps = 7
        }),
        ("liquidity capacity", |c| {
            c.liquidity.max_participation_rate_bps = 500
        }),
        ("sizing target qty", |c| c.sizing.target_qty = 2),
        ("sizing max target qty", |c| {
            c.sizing.max_target_qty = Some(5)
        }),
        ("sizing max notional", |c| {
            c.sizing.max_position_notional_usd = Some(1_000_000)
        }),
        ("quantity semantics", |c| {
            c.quantity_semantics = QuantitySemanticsId::FractionalQtyMicrosV1
        }),
        ("gross exposure", |c| c.max_gross_exposure_mult_micros += 1),
        ("daily loss limit", |c| c.daily_loss_limit_micros += 1),
        ("max drawdown limit", |c| c.max_drawdown_limit_micros += 1),
        ("reject storm", |c| c.reject_storm_max_rejects += 1),
        ("pdt", |c| c.pdt_enabled = !c.pdt_enabled),
        ("kill switch", |c| {
            c.kill_switch_flattens = !c.kill_switch_flattens
        }),
        ("shadow mode", |c| c.shadow_mode = true),
        ("bar history length", |c| c.bar_history_len += 1),
        ("initial capital", |c| c.initial_cash_micros /= 2),
        ("integrity enabled", |c| c.integrity_enabled = false),
        ("integrity stale threshold", |c| {
            c.integrity_stale_threshold_ticks = 120
        }),
        ("integrity gap tolerance", |c| {
            c.integrity_gap_tolerance_bars = 0
        }),
        ("integrity feed disagreement", |c| {
            c.integrity_enforce_feed_disagreement = false
        }),
        ("integrity calendar", |c| {
            c.integrity_calendar = mqk_integrity::CalendarSpec::NyseWeekdaysStartAnchored
        }),
        ("corporate action policy", |c| {
            c.corporate_action_policy = mqk_backtest::CorporateActionPolicy::Allow
        }),
    ];
    for (name, mutate) in cases {
        let mut mutated = cfg.clone();
        mutate(&mut mutated);
        assert_ne!(mutated, cfg, "{name}: mutation must change the config");
        let report = backtest_report(&mutated, None, &bars);
        let capital = mutated.initial_cash_micros;
        let res = promote_binding(&ev, &report, capital);
        assert!(
            res.is_err(),
            "{name}: mismatched config must be refused, got {res:?}"
        );
    }
}

/// Instrument economics live OUTSIDE `BacktestConfig`; the canonical run's
/// economics must still match the scanned candidate's (equity multiplier 1).
#[test]
fn instrument_economics_mismatch_is_refused() {
    let bars = bars();
    let cfg = canonical_cfg();
    let fx = fixture(scan_candidate(&cfg, &bars));
    let ev = validated(&fx);

    for (name, economics) in [
        (
            "contract multiplier",
            BacktestInstrumentEconomics::new(50, None, None).unwrap(),
        ),
        (
            "margin scaffold",
            BacktestInstrumentEconomics::new(1, Some(1_000_000_000), Some(800_000_000)).unwrap(),
        ),
    ] {
        let report = backtest_report(&cfg, Some(economics), &bars);
        assert_eq!(
            report.config_id,
            cfg.config_id(),
            "{name}: config unchanged"
        );
        let res = promote_binding(&ev, &report, CAPITAL);
        assert!(
            res.is_err(),
            "{name}: mismatched economics must be refused, got {res:?}"
        );
    }
}

/// Data / fingerprint / execution-model / capital identities of the canonical
/// evidence must each still be bound.
#[test]
fn data_fingerprint_execution_model_and_capital_mismatches_are_refused() {
    let bars = bars();
    let cfg = canonical_cfg();
    let fx = fixture(scan_candidate(&cfg, &bars));
    let ev = validated(&fx);
    let good = backtest_report(&cfg, None, &bars);
    promote_binding(&ev, &good, CAPITAL).expect("control must pass");

    // Different bars under the same config.
    let other_bars = backtest_report(&cfg, None, &bars_with_slope(60_000));
    assert!(
        promote_binding(&ev, &other_bars, CAPITAL).is_err(),
        "different data"
    );

    let mut r = good.clone();
    r.strategy_semantic_fingerprint = "f".repeat(64);
    assert!(
        promote_binding(&ev, &r, CAPITAL).is_err(),
        "different semantic fingerprint"
    );

    let mut r = good.clone();
    r.execution_model_id = "same_bar_v0".to_string();
    assert!(
        promote_binding(&ev, &r, CAPITAL).is_err(),
        "different execution model"
    );

    let mut r = good.clone();
    r.run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"other-run");
    assert!(
        promote_binding(&ev, &r, CAPITAL).is_err(),
        "different run identity"
    );

    assert!(
        promote_binding(&ev, &good, CAPITAL / 2).is_err(),
        "different capital basis"
    );
}

/// A forged / legacy-shaped row that does not carry a parseable candidate
/// config or run identity can never satisfy the binding.
#[test]
fn malformed_or_absent_candidate_identity_is_refused() {
    let bars = bars();
    let cfg = canonical_cfg();
    let fx = fixture(scan_candidate(&cfg, &bars));
    let good = validated(&fx);
    let report = backtest_report(&cfg, None, &bars);
    promote_binding(&good, &report, CAPITAL).expect("control must pass");

    let mut e = good.clone();
    e.benchmark_v2.as_mut().unwrap().candidate_config_id = "not-a-uuid".to_string();
    assert!(
        promote_binding(&e, &report, CAPITAL).is_err(),
        "unparseable config id"
    );

    let mut e = good.clone();
    e.benchmark_v2.as_mut().unwrap().candidate_run_id =
        "00000000-0000-0000-0000-000000000000".to_string();
    assert!(
        promote_binding(&e, &report, CAPITAL).is_err(),
        "other run id"
    );

    // Nil / unusable identities on either side never "match" each other.
    let nil = uuid::Uuid::nil().to_string();
    let mut r = report.clone();
    r.config_id = uuid::Uuid::nil();
    assert!(
        promote_binding(&good, &r, CAPITAL).is_err(),
        "nil backtest config id"
    );
    let mut both_nil = good.clone();
    both_nil.benchmark_v2.as_mut().unwrap().candidate_config_id = nil.clone();
    assert!(
        promote_binding(&both_nil, &r, CAPITAL).is_err(),
        "nil == nil config id"
    );

    let mut r = report.clone();
    r.run_id = uuid::Uuid::nil();
    let mut both_nil = good.clone();
    both_nil.benchmark_v2.as_mut().unwrap().candidate_run_id = nil;
    assert!(
        promote_binding(&both_nil, &r, CAPITAL).is_err(),
        "nil == nil run id"
    );

    let mut garbage = good.clone();
    garbage.benchmark_v2.as_mut().unwrap().candidate_run_id = "garbage".to_string();
    assert!(
        promote_binding(&garbage, &report, CAPITAL).is_err(),
        "unparseable run id"
    );
}

/// Wiring guard (supplement, not a substitute for the tests above): the
/// production route must hand the canonical Backtest bundle's identity to the
/// binding.
#[test]
fn promotion_route_feeds_the_backtest_bundle_identity_to_the_binding() {
    let src = include_str!("../src/routes/strategy_promotions.rs").replace("\r\n", "\n");
    let enforce = src
        .find("enforce_native_review_benchmark_binding(")
        .expect("native review benchmark binding call");
    let decide = src
        .find("mqk_promotion::evaluate_promotion(")
        .expect("canonical promotion decision");
    let call: String = src[enforce..decide].split_whitespace().collect();
    assert!(call.contains("BacktestEvidenceIdentity::from_bundle(&backtest_bundle"));
}
