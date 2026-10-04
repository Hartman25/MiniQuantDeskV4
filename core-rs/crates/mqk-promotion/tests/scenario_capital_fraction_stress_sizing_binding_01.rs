//! IR-B02-02: Promotion-time binding of the capital-fraction stress-sizing provenance.
//!
//! `resolve_backtest_evidence` must not hand `evaluate_promotion` a capital-fraction candidate whose
//! P7A/P7B stress evidence lacks, malforms, or contradicts its stress-sizing provenance relative to the
//! candidate's OWN authenticated baseline sizing contract. The check is policy-neutral: no particular
//! stress fraction is required (a 250 bps stress of a 1000 bps baseline is as acceptable as 500 bps).
//! Defects surface through the list `evaluate_promotion` already refuses on.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestReport, RobustnessScenarioOutcome,
    SizingEntryProvenance, SizingPolicy, SizingProvenance,
};
use mqk_promotion::resolve_backtest_evidence;
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};
use serde_json::{json, Value};

const M: i64 = 1_000_000;
const CAPITAL: i64 = 100_000_000_000;
const BASELINE_BPS: i64 = 1_000;
static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct BuyHoldSell {
    bar_idx: u64,
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("CfStressBinding", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        let qty = if self.bar_idx < 3 { 1 } else { 0 };
        StrategyOutput::new(vec![TargetPosition::whole("ES", qty)])
    }
}

fn bars() -> Vec<BacktestBar> {
    [500, 501, 502, 503]
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let px = p * M;
            BacktestBar::new("ES", 1_700_000_060 + 60 * i as i64, px, px, px, px, 1_000)
        })
        .collect()
}

fn hex64(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

/// Authenticated Backtest caps `(max_target_qty, max_position_notional_usd)` of the default fixture.
const CAPS: (Option<i64>, Option<i64>) = (Some(1_000), Some(50_000));

type Caps = (Option<i64>, Option<i64>);

/// A stress-sizing echo shaped exactly like the Python replay emits, consistent with the baseline.
fn consistent_echo(baseline_fp: &str, stress_bps: i64) -> Value {
    consistent_echo_caps(baseline_fp, stress_bps, CAPS)
}

fn consistent_echo_caps(baseline_fp: &str, stress_bps: i64, caps: Caps) -> Value {
    json!({
        "baseline_caps": {
            "max_target_qty": caps.0,
            "max_position_notional_usd": caps.1,
        },
        "scenario_id": "half_exposure_test_v1",
        "policy_id": "fixed_initial_capital_fraction_v1",
        "allocation_fraction_bps": stress_bps,
        "baseline_allocation_fraction_bps": BASELINE_BPS,
        "initial_allocated_capital_micros": CAPITAL,
        "nominal_entry_budget_micros": CAPITAL * stress_bps / 10_000,
        "baseline_nominal_entry_budget_micros": CAPITAL * BASELINE_BPS / 10_000,
        "baseline_semantic_fingerprint": baseline_fp,
        "stress_semantic_fingerprint": hex64('e'),
        "caps_unchanged_from_baseline": true,
        "is_a_trial": false,
        "stress_native_signals_csv_sha256": hex64('a'),
        "stress_native_signals_meta_sha256": hex64('b'),
        "stress_oos_predictions_csv_sha256": hex64('c'),
    })
}

/// `stress_spec` may carry `max_drawdown_ceiling` (moved to the evidence top level, as the Python
/// replay emits it) and the execution-pricing echo; absent ones default to the registered values.
fn p7a_outcome(mut stress_spec: Value) -> RobustnessScenarioOutcome {
    let obj = stress_spec.as_object_mut().expect("stress_spec object");
    let ceiling = obj.remove("max_drawdown_ceiling").unwrap_or(json!(0.4));
    obj.entry("execution_pricing_slippage_bps")
        .or_insert(json!(15));
    obj.entry("execution_pricing_volatility_mult_bps")
        .or_insert(json!(10));
    RobustnessScenarioOutcome {
        name: mqk_backtest::P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("cf_binding_trial".to_string()),
        evidence: Some(json!({
            "status": "evaluated",
            "stress_spec": stress_spec,
            "max_drawdown_ceiling": ceiling,
        })),
    }
}

/// Persist a full candidate (report, stress suite, gauntlet + the p7a scenario) and resolve it.
/// `capital_fraction` selects the recorded sizing policy; `stress_spec` is the p7a `stress_spec`.
fn resolve_candidate(
    label: &str,
    capital_fraction: bool,
    stress_spec: impl FnOnce(&str) -> Value,
) -> Vec<String> {
    resolve_candidate_caps(label, capital_fraction, CAPS, stress_spec)
}

/// `caps` are the Backtest sizing caps recorded in the authenticated canonical report.
fn resolve_candidate_caps(
    label: &str,
    capital_fraction: bool,
    caps: Caps,
    stress_spec: impl FnOnce(&str) -> Value,
) -> Vec<String> {
    resolve_bundle(label, capital_fraction, caps, stress_spec)
        .robustness_evidence
        .p7a_p7b_economic_replay_stress_missing_required_evidence_fields
        .into_iter()
        .filter(|f| f.starts_with("stress_spec.stress_sizing"))
        .collect()
}

fn resolve_bundle(
    label: &str,
    capital_fraction: bool,
    caps: Caps,
    stress_spec: impl FnOnce(&str) -> Value,
) -> mqk_promotion::BacktestEvidenceBundle {
    let mut config = BacktestConfig::test_defaults();
    config.max_gross_exposure_mult_micros = 100_000_000;
    config.sizing.max_target_qty = caps.0;
    config.sizing.max_position_notional_usd = caps.1;
    let bars = bars();
    let mut engine = BacktestEngine::new(config.clone());
    engine
        .add_strategy(Box::new(BuyHoldSell { bar_idx: 0 }))
        .unwrap();
    let engine_report = engine.run(&bars).expect("engine.run");
    let report = if capital_fraction {
        BacktestReport {
            sizing_provenance: SizingProvenance {
                policy: SizingPolicy::capital_fraction_v1(BASELINE_BPS).unwrap(),
                entries: vec![SizingEntryProvenance {
                    symbol: "ES".into(),
                    reference_bar_end_ts: 1_700_000_060,
                    allocation_fraction_bps: BASELINE_BPS,
                    initial_allocated_capital_micros: CAPITAL,
                    position_budget_micros: CAPITAL * BASELINE_BPS / 10_000,
                    causal_reference_price_micros: 500 * M,
                    uncapped_target_qty_micros: 20 * M,
                    resolved_target_qty_micros: 20 * M,
                    capped_by: "none".into(),
                }],
                refusals: vec![],
            },
            ..engine_report
        }
    } else {
        engine_report
    };

    let seq = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let root: PathBuf =
        std::env::temp_dir().join(format!("mqk_cfsb01_{label}_{}_{seq}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let init = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: &root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "cfsb01_test_git_hash",
        config_hash: &report.config_id.to_string(),
        host_fingerprint: "cfsb01_test_host",
        now_utc: chrono::Utc::now(),
    })
    .expect("init_run_artifacts");
    mqk_artifacts::write_backtest_report(&init.run_dir, &report, config.initial_cash_micros)
        .expect("write_backtest_report");
    let stress = mqk_backtest::run_backtest_stress_suite(&report, &config, &bars, || {
        Box::new(BuyHoldSell { bar_idx: 0 })
    });
    mqk_artifacts::write_canonical_stress_suite(&init.run_dir, &stress).expect("stress suite");
    let gauntlet = mqk_backtest::run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(BuyHoldSell { bar_idx: 0 })
    })
    .merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("cf_binding_trial".to_string()),
        evidence: None,
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&init.run_dir, &gauntlet).expect("gauntlet");
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init.run_dir,
        &p7a_outcome(stress_spec(&report.strategy_semantic_fingerprint)),
    )
    .expect("finalize p7a stress");

    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");
    let _ = fs::remove_dir_all(&root);
    assert_eq!(
        (
            bundle.report.sizing.max_target_qty,
            bundle.report.sizing.max_position_notional_usd
        ),
        caps,
        "fixture: the authenticated report must carry the intended baseline caps"
    );
    bundle
}

fn capital_fraction_defects(mutate: impl FnOnce(&mut Value)) -> Vec<String> {
    resolve_candidate("cf", true, |fp| {
        let mut echo = consistent_echo(fp, 500);
        mutate(&mut echo);
        json!({ "stress_sizing": echo })
    })
}

#[test]
fn cfsb01a_consistent_stress_sizing_is_accepted_at_any_strictly_adverse_fraction() {
    for stress_bps in [500, 250, 999] {
        let defects = resolve_candidate(
            "cf_ok",
            true,
            |fp| json!({ "stress_sizing": consistent_echo(fp, stress_bps) }),
        );
        assert!(defects.is_empty(), "{stress_bps} bps: {defects:?}");
    }
}

#[test]
fn cfsb01b_a_capital_fraction_candidate_with_no_stress_sizing_is_flagged() {
    let none = resolve_candidate("cf_none", true, |_| json!({ "max_target_qty": null }));
    assert_eq!(none, vec!["stress_spec.stress_sizing".to_string()]);
    let not_object = resolve_candidate("cf_str", true, |_| json!({ "stress_sizing": "half" }));
    assert_eq!(not_object, vec!["stress_spec.stress_sizing".to_string()]);
}

/// (label, mutation of the consistent echo, the defect field it must be reported under)
type Case = (&'static str, Box<dyn FnOnce(&mut Value)>, &'static str);

#[test]
fn cfsb01c_inconsistent_or_malformed_stress_sizing_is_flagged_field_by_field() {
    let cases: Vec<Case> = vec![
        (
            "wrong baseline bps",
            Box::new(|e| e["baseline_allocation_fraction_bps"] = json!(2_000)),
            "baseline_allocation_fraction_bps",
        ),
        (
            "stress equals baseline",
            Box::new(|e| e["allocation_fraction_bps"] = json!(BASELINE_BPS)),
            "allocation_fraction_bps",
        ),
        (
            "stress above baseline",
            Box::new(|e| e["allocation_fraction_bps"] = json!(1_500)),
            "allocation_fraction_bps",
        ),
        (
            "stress zero",
            Box::new(|e| e["allocation_fraction_bps"] = json!(0)),
            "allocation_fraction_bps",
        ),
        (
            "stress missing",
            Box::new(|e| {
                e.as_object_mut().unwrap().remove("allocation_fraction_bps");
            }),
            "allocation_fraction_bps",
        ),
        (
            "wrong capital",
            Box::new(|e| e["initial_allocated_capital_micros"] = json!(CAPITAL / 2)),
            "initial_allocated_capital_micros",
        ),
        (
            "wrong stress budget",
            Box::new(|e| e["nominal_entry_budget_micros"] = json!(1)),
            "nominal_entry_budget_micros",
        ),
        (
            "wrong baseline budget",
            Box::new(|e| e["baseline_nominal_entry_budget_micros"] = json!(1)),
            "baseline_nominal_entry_budget_micros",
        ),
        (
            "wrong policy",
            Box::new(|e| e["policy_id"] = json!("fixed_quantity_v1")),
            "policy_id",
        ),
        (
            "other candidate fingerprint",
            Box::new(|e| e["baseline_semantic_fingerprint"] = json!(hex64('9'))),
            "baseline_semantic_fingerprint",
        ),
        (
            "empty baseline fingerprint",
            Box::new(|e| e["baseline_semantic_fingerprint"] = json!("")),
            "baseline_semantic_fingerprint",
        ),
        (
            "stress fingerprint not re-resolved",
            Box::new(|e| {
                let fp = e["baseline_semantic_fingerprint"].clone();
                e["stress_semantic_fingerprint"] = fp;
            }),
            "stress_semantic_fingerprint",
        ),
        (
            "stress fingerprint malformed",
            Box::new(|e| e["stress_semantic_fingerprint"] = json!("xyz")),
            "stress_semantic_fingerprint",
        ),
        (
            "caps changed",
            Box::new(|e| e["caps_unchanged_from_baseline"] = json!(false)),
            "caps_unchanged_from_baseline",
        ),
        (
            "baseline_caps absent",
            Box::new(|e| {
                e.as_object_mut().unwrap().remove("baseline_caps");
            }),
            "baseline_caps",
        ),
        (
            "baseline_caps not an object",
            Box::new(|e| e["baseline_caps"] = json!("none")),
            "baseline_caps",
        ),
        (
            "baseline_caps null",
            Box::new(|e| e["baseline_caps"] = Value::Null),
            "baseline_caps",
        ),
        (
            "max_target_qty absent",
            Box::new(|e| {
                e["baseline_caps"]
                    .as_object_mut()
                    .unwrap()
                    .remove("max_target_qty");
            }),
            "baseline_caps.max_target_qty",
        ),
        (
            "max_target_qty wrong integer",
            Box::new(|e| e["baseline_caps"]["max_target_qty"] = json!(999)),
            "baseline_caps.max_target_qty",
        ),
        (
            "max_target_qty null against Some baseline",
            Box::new(|e| e["baseline_caps"]["max_target_qty"] = Value::Null),
            "baseline_caps.max_target_qty",
        ),
        (
            "max_target_qty float",
            Box::new(|e| e["baseline_caps"]["max_target_qty"] = json!(1000.0)),
            "baseline_caps.max_target_qty",
        ),
        (
            "max_target_qty numeric string",
            Box::new(|e| e["baseline_caps"]["max_target_qty"] = json!("1000")),
            "baseline_caps.max_target_qty",
        ),
        (
            "max_position_notional_usd absent",
            Box::new(|e| {
                e["baseline_caps"]
                    .as_object_mut()
                    .unwrap()
                    .remove("max_position_notional_usd");
            }),
            "baseline_caps.max_position_notional_usd",
        ),
        (
            "max_position_notional_usd wrong integer",
            Box::new(|e| e["baseline_caps"]["max_position_notional_usd"] = json!(49_999)),
            "baseline_caps.max_position_notional_usd",
        ),
        (
            "max_position_notional_usd null against Some baseline",
            Box::new(|e| e["baseline_caps"]["max_position_notional_usd"] = Value::Null),
            "baseline_caps.max_position_notional_usd",
        ),
        (
            "max_position_notional_usd float",
            Box::new(|e| e["baseline_caps"]["max_position_notional_usd"] = json!(50_000.0)),
            "baseline_caps.max_position_notional_usd",
        ),
        (
            "max_position_notional_usd numeric string",
            Box::new(|e| e["baseline_caps"]["max_position_notional_usd"] = json!("50000")),
            "baseline_caps.max_position_notional_usd",
        ),
        (
            "stress claims to be a trial",
            Box::new(|e| e["is_a_trial"] = json!(true)),
            "is_a_trial",
        ),
        (
            "no scenario id",
            Box::new(|e| e["scenario_id"] = json!(" ")),
            "scenario_id",
        ),
        (
            "no stream hash",
            Box::new(|e| e["stress_native_signals_csv_sha256"] = json!("")),
            "stress_native_signals_csv_sha256",
        ),
        (
            "no oos hash",
            Box::new(|e| {
                e.as_object_mut()
                    .unwrap()
                    .remove("stress_oos_predictions_csv_sha256");
            }),
            "stress_oos_predictions_csv_sha256",
        ),
    ];
    for (label, mutate, field) in cases {
        let defects = capital_fraction_defects(mutate);
        assert!(
            defects.contains(&format!("stress_spec.stress_sizing.{field}")),
            "{label}: expected {field} in {defects:?}"
        );
    }
}

#[test]
fn cfsb01e_caps_unchanged_boolean_cannot_spoof_the_actual_cap_binding() {
    // The echo keeps `caps_unchanged_from_baseline = true`; only one actual cap differs.
    for (label, key, wrong) in [
        ("qty", "max_target_qty", json!(1_001)),
        ("notional", "max_position_notional_usd", json!(1)),
    ] {
        let defects = capital_fraction_defects(|e| e["baseline_caps"][key] = wrong);
        assert_eq!(
            defects,
            vec![format!("stress_spec.stress_sizing.baseline_caps.{key}")],
            "{label}: only the actual cap must be flagged while the boolean still claims true"
        );
    }
}

#[test]
fn cfsb01f_none_baseline_caps_accept_only_explicit_json_null() {
    let run = |caps: Caps, mutate: fn(&mut Value)| {
        resolve_candidate_caps("cf_none_caps", true, caps, |fp| {
            let mut echo = consistent_echo_caps(fp, 500, caps);
            mutate(&mut echo);
            json!({ "stress_sizing": echo })
        })
    };
    let q = "stress_spec.stress_sizing.baseline_caps.max_target_qty".to_string();
    let n = "stress_spec.stress_sizing.baseline_caps.max_position_notional_usd".to_string();

    // Correct nulls / mixed Some+None combinations pass, at several adverse fractions' worth of echo.
    for caps in [(None, None), (Some(1_000), None), (None, Some(50_000))] {
        assert!(run(caps, |_| {}).is_empty(), "{caps:?}");
    }
    // Missing key is not null.
    assert_eq!(
        run((None, None), |e| {
            let c = e["baseline_caps"].as_object_mut().unwrap();
            c.remove("max_target_qty");
            c.remove("max_position_notional_usd");
        }),
        vec![q.clone(), n.clone()]
    );
    // A concrete value or the string "null" is not null.
    assert_eq!(
        run((None, None), |e| {
            e["baseline_caps"]["max_target_qty"] = json!(1_000);
            e["baseline_caps"]["max_position_notional_usd"] = json!("null");
        }),
        vec![q.clone(), n.clone()]
    );
    assert_eq!(
        run((Some(1_000), None), |e| e["baseline_caps"]
            ["max_position_notional_usd"] =
            json!(0)),
        vec![n.clone()]
    );
    assert_eq!(
        run((None, Some(50_000)), |e| e["baseline_caps"]
            ["max_target_qty"] = json!(0)),
        vec![q]
    );
}

#[test]
fn cfsb01d_a_fixed_quantity_candidate_is_unaffected() {
    // Historical cap-mode candidates carry no stress_sizing and must not be asked for one.
    let defects = resolve_candidate("fixed", false, |_| json!({ "max_target_qty": 1 }));
    assert!(defects.is_empty(), "{defects:?}");
}

// ---------------------------------------------------------------------------
// Predeclared stress authority: the registered Research trial fixes the stress
// scenario; Promotion compares the candidate's stress evidence with it.
// ---------------------------------------------------------------------------

const TEST_SCENARIO: &str = "half_exposure_test_v1";

fn native_identity(capital_sizing: Option<i64>, stress_contract: Option<Value>) -> String {
    let mut source = json!({
        "kind": "native_strategy_signal_stream_v2",
        "symbol": "ES",
        "semantic_fingerprint": hex64('a'),
        "target_semantics": "absolute_whole_share_target_v1",
        "required_history_bars": 50,
    });
    if let Some(bps) = capital_sizing {
        source["capital_sizing"] = json!({
            "policy_id": "fixed_initial_capital_fraction_v1",
            "allocation_fraction_bps": bps,
            "initial_allocated_capital_micros": CAPITAL,
            "max_target_qty": CAPS.0,
            "max_position_notional_usd": CAPS.1,
        });
    }
    if let Some(contract) = stress_contract {
        source["stress_contract"] = contract;
    }
    json!({
        "signal_source": source,
        "economic_protocol": {"signal_policy": {
            "direction_policy": "native_exact_target_qty_v1",
            "sizing": "exact_native_target_qty_v1",
        }},
    })
    .to_string()
}

fn contract(scenario: &str, bps: i64) -> Value {
    json!({
        "schema_version": "p7a_p7b_stress_contract_v1",
        "scenario_id": scenario,
        "allocation_fraction_bps": bps,
        "stress_execution_slippage_bps": 15,
        "stress_execution_volatility_mult_bps": 10,
        "max_drawdown_ceiling_bps": 4000,
    })
}

fn contract_with(field: &str, value: Value) -> Value {
    let mut c = contract(TEST_SCENARIO, 500);
    c[field] = value;
    c
}

fn registered(identity: String) -> mqk_promotion::VerifiedPromotionOosEvidence {
    common::valid_oos_evidence_for_testing_with_identity("cf_binding_trial", &identity)
}

fn cf_bundle(echo: impl FnOnce(&str) -> Value) -> mqk_promotion::BacktestEvidenceBundle {
    cf_bundle_spec(echo, |_| {})
}

/// `patch` edits the p7a `stress_spec` (execution-pricing echo, `max_drawdown_ceiling`).
fn cf_bundle_spec(
    echo: impl FnOnce(&str) -> Value,
    patch: impl FnOnce(&mut Value),
) -> mqk_promotion::BacktestEvidenceBundle {
    resolve_bundle("cf_contract", true, CAPS, |fp| {
        let mut spec = json!({ "stress_sizing": echo(fp) });
        patch(&mut spec);
        spec
    })
}

#[test]
fn cfsb02a_the_registered_contract_is_accepted_only_when_the_evidence_equals_it() {
    let bundle = cf_bundle(|fp| consistent_echo(fp, 500));
    let oos = registered(native_identity(
        Some(1000),
        Some(contract(TEST_SCENARIO, 500)),
    ));
    assert_eq!(
        oos.registered_stress_contract().map(|c| (
            c.scenario_id.as_str(),
            c.allocation_fraction_bps,
            c.stress_execution_slippage_bps,
            c.stress_execution_volatility_mult_bps,
            c.max_drawdown_ceiling_bps
        )),
        Some((TEST_SCENARIO, 500, 15, 10, 4000))
    );
    assert_eq!(
        mqk_promotion::verify_registered_stress_contract(&bundle, &oos),
        Ok(())
    );
}

#[test]
fn cfsb02b_a_different_scenario_or_fraction_than_the_registered_one_is_refused() {
    // A friendlier (999 bps) and a harsher-than-registered (250 bps) evidence stress both differ from 500.
    for echo_bps in [999, 250] {
        let bundle = cf_bundle(|fp| consistent_echo(fp, echo_bps));
        let oos = registered(native_identity(
            Some(1000),
            Some(contract(TEST_SCENARIO, 500)),
        ));
        let err = mqk_promotion::verify_registered_stress_contract(&bundle, &oos).unwrap_err();
        assert!(err.contains("allocation_fraction_bps"), "{echo_bps}: {err}");
    }
    let bundle = cf_bundle(|fp| consistent_echo(fp, 500));
    let oos = registered(native_identity(
        Some(1000),
        Some(contract("some_other_scenario", 500)),
    ));
    assert!(
        mqk_promotion::verify_registered_stress_contract(&bundle, &oos)
            .unwrap_err()
            .contains("scenario_id")
    );
}

/// Each registered field is compared on its own: a single differing field, with every other field
/// equal, must refuse (kills the removal of any one comparison).
#[test]
fn cfsb02g_a_mismatch_on_any_single_registered_field_is_refused() {
    let cases: Vec<(&str, Value)> = vec![
        ("scenario_id", json!("other_scenario")),
        ("allocation_fraction_bps", json!(499)),
        ("stress_execution_slippage_bps", json!(16)),
        ("stress_execution_volatility_mult_bps", json!(11)),
        ("max_drawdown_ceiling_bps", json!(3500)),
    ];
    let bundle = cf_bundle(|fp| consistent_echo(fp, 500));
    for (field, value) in cases {
        let oos = registered(native_identity(
            Some(1000),
            Some(contract_with(field, value)),
        ));
        let err = mqk_promotion::verify_registered_stress_contract(&bundle, &oos).unwrap_err();
        assert!(err.contains(field), "{field}: {err}");
    }
}

/// The evidence side of every execution input: an echo that differs from the registered contract
/// in exactly one execution-stress input, with the contract untouched, is refused.
#[test]
fn cfsb02h_an_execution_stress_echo_that_differs_from_the_registered_value_is_refused() {
    let oos = registered(native_identity(
        Some(1000),
        Some(contract(TEST_SCENARIO, 500)),
    ));
    type Patch = fn(&mut Value);
    let cases: [(&str, Patch); 3] = [
        ("stress_execution_slippage_bps", |s| {
            s["execution_pricing_slippage_bps"] = json!(14)
        }),
        ("stress_execution_volatility_mult_bps", |s| {
            s["execution_pricing_volatility_mult_bps"] = json!(9)
        }),
        ("max_drawdown_ceiling_bps", |s| {
            s["max_drawdown_ceiling"] = json!(0.45)
        }),
    ];
    for (needle, patch) in cases {
        let bundle = cf_bundle_spec(|fp| consistent_echo(fp, 500), patch);
        let err = mqk_promotion::verify_registered_stress_contract(&bundle, &oos).unwrap_err();
        assert!(err.contains(needle), "{needle}: {err}");
    }
}

#[test]
fn cfsb02c_a_capital_fraction_candidate_without_a_registered_contract_is_refused() {
    let bundle = cf_bundle(|fp| consistent_echo(fp, 500));
    let oos = registered(native_identity(Some(1000), None));
    assert!(oos.registered_stress_contract().is_none());
    assert!(
        mqk_promotion::verify_registered_stress_contract(&bundle, &oos)
            .unwrap_err()
            .contains("predeclares no stress contract")
    );
    // A legacy trial with no native signal source at all is not a way around it.
    let legacy = common::valid_oos_evidence_for_testing("cf_binding_trial");
    assert!(mqk_promotion::verify_registered_stress_contract(&bundle, &legacy).is_err());
}

#[test]
fn cfsb02d_a_candidate_with_no_readable_stress_scenario_is_refused() {
    let oos = registered(native_identity(
        Some(1000),
        Some(contract(TEST_SCENARIO, 500)),
    ));
    for missing in ["scenario_id", "allocation_fraction_bps"] {
        let bundle = cf_bundle(|fp| {
            let mut echo = consistent_echo(fp, 500);
            echo.as_object_mut().unwrap().remove(missing);
            echo
        });
        assert!(bundle.stress_echo.is_none(), "{missing}");
        assert!(
            mqk_promotion::verify_registered_stress_contract(&bundle, &oos)
                .unwrap_err()
                .contains("no readable stress scenario")
        );
    }
    type Patch = fn(&mut Value);
    let spec_defects: [(&str, Patch); 4] = [
        ("no slippage", |s| {
            s["execution_pricing_slippage_bps"] = Value::Null
        }),
        ("float volatility", |s| {
            s["execution_pricing_volatility_mult_bps"] = json!(10.5)
        }),
        ("no ceiling", |s| s["max_drawdown_ceiling"] = Value::Null),
        ("non-bps ceiling", |s| {
            s["max_drawdown_ceiling"] = json!(0.40005)
        }),
    ];
    for (label, patch) in spec_defects {
        let bundle = cf_bundle_spec(|fp| consistent_echo(fp, 500), patch);
        assert!(bundle.stress_echo.is_none(), "{label}");
        assert!(
            mqk_promotion::verify_registered_stress_contract(&bundle, &oos).is_err(),
            "{label}"
        );
    }
}

#[test]
fn cfsb02e_a_fixed_quantity_candidate_carries_no_contract() {
    let fixed = resolve_bundle("fq_contract", false, CAPS, |_| json!({}));
    let without = registered(native_identity(None, None));
    assert_eq!(
        mqk_promotion::verify_registered_stress_contract(&fixed, &without),
        Ok(())
    );
    let with = registered(native_identity(
        Some(1000),
        Some(contract(TEST_SCENARIO, 500)),
    ));
    assert!(
        mqk_promotion::verify_registered_stress_contract(&fixed, &with)
            .unwrap_err()
            .contains("not capital-fraction sized")
    );
}

#[test]
fn cfsb02f_a_malformed_registered_contract_fails_the_research_authority() {
    let bad_json = |v: Value| native_identity(Some(1000), Some(v));
    let bad: Vec<(&str, String)> = vec![
        (
            "not strictly below baseline",
            bad_json(contract(TEST_SCENARIO, 1000)),
        ),
        ("zero", bad_json(contract(TEST_SCENARIO, 0))),
        ("empty scenario", bad_json(contract("  ", 500))),
        (
            "no baseline",
            native_identity(None, Some(contract(TEST_SCENARIO, 500))),
        ),
        (
            "float bps",
            bad_json(contract_with("allocation_fraction_bps", json!(500.5))),
        ),
        (
            "bool bps",
            bad_json(contract_with("allocation_fraction_bps", json!(true))),
        ),
        ("extra key", bad_json(contract_with("x", json!(1)))),
        (
            "missing key",
            bad_json(json!({"scenario_id": TEST_SCENARIO})),
        ),
        (
            "superseded 2-key contract",
            bad_json(json!({"scenario_id": TEST_SCENARIO, "allocation_fraction_bps": 500})),
        ),
        (
            "unknown schema_version",
            bad_json(contract_with("schema_version", json!("v0"))),
        ),
        (
            "float slippage",
            bad_json(contract_with("stress_execution_slippage_bps", json!(15.5))),
        ),
        (
            "negative volatility",
            bad_json(contract_with(
                "stress_execution_volatility_mult_bps",
                json!(-1),
            )),
        ),
        (
            "zero ceiling",
            bad_json(contract_with("max_drawdown_ceiling_bps", json!(0))),
        ),
        (
            "ceiling above 100%",
            bad_json(contract_with("max_drawdown_ceiling_bps", json!(10001))),
        ),
        ("not an object", bad_json(json!("half"))),
    ];
    for (label, identity) in bad {
        let errs =
            common::try_valid_oos_evidence_for_testing_with_identity("cf_binding_trial", &identity)
                .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("stress_contract is invalid")),
            "{label}: {errs:?}"
        );
    }
}
