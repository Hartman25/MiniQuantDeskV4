//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` / P7A-P7B-ECONOMIC-REPLAY-STRESS-01 --
//! genuine conservative P7A/P7B execution/capacity stress, via cross-language
//! orchestration of the FROZEN, accepted Python economic-walkforward engine.
//!
//! This module never re-implements P7A execution-pricing parity or P7B
//! weight-to-share translation in Rust (forbidden -- see
//! `crate::robustness_gauntlet`'s own module docs and CLAUDE.md's
//! "statistical research" rule). It shells out to `research-py`'s
//! `mqk_research.ml.p7a_p7b_economic_replay_stress_cli`, a thin wrapper that
//! re-evaluates an ALREADY-REGISTERED Research trial's FROZEN OOS prediction
//! stream through the existing, accepted `run_economic_walkforward` entry
//! point under an explicit, caller-supplied stress configuration -- never a
//! new model training run, never a new Research trial.
//!
//! REPLAY AUTHORITY: `economic_walk_forward.json` (written by
//! `run_registered_economic_walkforward_eval`, REAL-RESEARCH-PROMOTION-E2E-
//! CLOSURE-01) durably records, via `file_record()` (`{path, bytes, sha256}`),
//! the EXACT `bars_csv`/`oos_predictions_csv`/`walk_forward_eval` inputs and
//! `bars_provenance` manifest that originally produced it. The Python CLI
//! re-verifies every one of those against the live filesystem (path exists,
//! byte count matches, sha256 matches) before replaying -- fails closed on
//! any missing/mutated input rather than refetching and assuming identity.
//!
//! FINAL-P7A-P7B-REPLAY-AUTHORITY-01: the caller must supply the EXACT
//! P7C-authorized `economic_eval_id` (`E`) this replay must bind to -- the
//! CLI resolves the succeeded attempt whose durable registry `result_id`
//! equals `E` (never "the latest successful attempt"), re-authenticates
//! that attempt's `economic_walk_forward.json` by recomputing its content
//! hash against that SAME durable authority (never trusting the file's own
//! self-declared id), and validates the caller-supplied stress knobs are
//! GENUINELY adverse relative to the verified baseline before replaying. A
//! trial whose economic evidence predates this durable-input recording,
//! never engaged the OFFICIAL P7A/P7B protocols, or does not bind to the
//! required `economic_eval_id` is reported `applicable: true, passed: false`
//! -- "MANDATORY MEANS MANDATORY": this required scenario can never
//! disappear from a promotion-grade P9 artifact via `applicable: false`,
//! unlike genuinely optional scenarios such as
//! `symbol_leave_one_out_scenario`'s "does not apply to this candidate".
//!
//! Deliberately kept OUT of
//! `crate::robustness_gauntlet::run_robustness_gauntlet` itself (a pure,
//! I/O-free-beyond-the-backtest-engine function) because this scenario needs
//! real subprocess + filesystem I/O and a completed Research trial, exactly
//! like [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`]. Callers
//! assembling the complete P9 evidence artifact call
//! [`p7a_p7b_economic_replay_stress_scenario`] separately and merge it in via
//! `RobustnessGauntletOutput::merge_dsr_pbo_sensitivity` (name-agnostic --
//! see that function's own docs).

use std::path::Path;
use std::process::Command;

use crate::robustness_gauntlet::RobustnessScenarioOutcome;

/// Scenario name this module reports under -- part of
/// `robustness_gauntlet::REQUIRED_ROBUSTNESS_SCENARIO_NAMES`.
pub const P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME: &str = "p7a_p7b_economic_replay_stress";

/// The exact accepted replay-stress protocol identity, mirroring
/// `mqk_research.ml.p7a_p7b_economic_replay_stress_cli.STRESS_PROTOCOL_ID`
/// byte-for-byte -- see that module's own `_run_replay_stress`, which emits
/// this under the evidence blob's `protocol_id` key on every "evaluated"
/// result. FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 4: checked by
/// `mqk_artifacts::RobustnessGauntletArtifact::
/// p7a_p7b_economic_replay_stress_missing_required_evidence_fields`.
pub const P7A_P7B_ECONOMIC_REPLAY_STRESS_PROTOCOL_ID: &str = "p7a_p7b_economic_replay_stress_v1";

/// Run the genuine P7A/P7B economic replay stress for `trial_id` against
/// `registry_db`, re-evaluating that trial's FROZEN OOS prediction stream
/// (never re-trained) through the real `run_economic_walkforward` machinery
/// under an explicit stress configuration.
///
/// `python_executable`/`research_py_root` are supplied by the caller, same
/// contract as [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`].
///
/// `economic_eval_id` is the EXACT, REQUIRED P7C-authorized economic result
/// (`E`) this replay must bind to -- never resolved by "latest successful
/// attempt". A mismatch between what the CLI resolves and this value fails
/// closed.
///
/// `expected_strategy_id` is the same cross-candidate authority check
/// `dsr_pbo_sensitivity_scenario` performs: the CLI resolves `trial_id`'s
/// own registered `strategy_id` and this function rejects a mismatch BEFORE
/// the result is ever merged into a candidate's P9 evidence.
///
/// `stress_execution_slippage_bps`/`stress_execution_volatility_mult_bps`
/// (P7A) and `stress_max_target_qty`/`stress_max_position_notional_usd`
/// (P7B) are the EXPLICIT, caller-supplied stress knobs -- no hidden
/// in-crate default (P9-P7A-P7B-REAL-STRESS-01's own discipline). Every
/// other field of the trial's baseline economic protocol (signal policy,
/// cost model, annualization) is carried through UNCHANGED from the
/// verified original.
///
/// `max_drawdown_ceiling` is the EXPLICIT, caller-supplied, required (no
/// default) conservative pass/fail tolerance -- must be finite and within
/// `[0, 1]` (a drawdown fraction). An out-of-range value is reported as a
/// genuine `applicable: true, passed: false` misconfiguration before any
/// subprocess is spawned, mirroring
/// `dsr_pbo_sensitivity_scenario`'s own threshold-validation discipline.
///
/// Fails closed: an invalid ceiling, a spawn failure, unparseable output, a
/// `strategy_id`/`economic_eval_id` mismatch, a baseline that never engaged
/// the official P7A/P7B protocols, missing/mutated replay inputs, a
/// non-genuine (not strictly adverse) stress configuration, or a genuine CLI
/// error (bad registry, unknown trial) all become
/// `applicable: true, passed: false` with the real reason -- MANDATORY MEANS
/// MANDATORY (see module docs): this scenario never reports
/// `applicable: false`.
#[allow(clippy::too_many_arguments)]
pub fn p7a_p7b_economic_replay_stress_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    expected_strategy_id: &str,
    stress_out_dir: &Path,
    stress_execution_slippage_bps: u32,
    stress_execution_volatility_mult_bps: u32,
    stress_max_target_qty: Option<u32>,
    stress_max_position_notional_usd: Option<f64>,
    max_drawdown_ceiling: f64,
) -> RobustnessScenarioOutcome {
    let name = P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string();
    let research_trial_id = Some(trial_id.to_string());

    // P9-P7A-P7B-REAL-STRESS-01: validate the caller-supplied policy
    // threshold BEFORE any I/O -- an invalid ceiling is a real
    // misconfiguration, never silently accepted.
    if !max_drawdown_ceiling.is_finite() || !(0.0..=1.0).contains(&max_drawdown_ceiling) {
        let reason = format!(
            "invalid max_drawdown_ceiling policy value: {max_drawdown_ceiling} (must be finite \
             and within [0, 1] -- a drawdown fraction)"
        );
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(reason.clone()),
            detail: reason,
            research_trial_id,
            evidence: None,
        };
    }

    let src_dir = research_py_root.join("src");
    let mut cmd = Command::new(python_executable);
    cmd.env("PYTHONPATH", &src_dir).args([
        "-m",
        "mqk_research.ml.p7a_p7b_economic_replay_stress_cli",
        "--registry-db",
        &registry_db.display().to_string(),
        "--trial-id",
        trial_id,
        "--economic-eval-id",
        economic_eval_id,
        "--stress-out-dir",
        &stress_out_dir.display().to_string(),
        "--stress-execution-slippage-bps",
        &stress_execution_slippage_bps.to_string(),
        "--stress-execution-volatility-mult-bps",
        &stress_execution_volatility_mult_bps.to_string(),
        "--max-drawdown-ceiling",
        &max_drawdown_ceiling.to_string(),
    ]);
    if let Some(qty) = stress_max_target_qty {
        cmd.args(["--stress-max-target-qty", &qty.to_string()]);
    }
    if let Some(notional) = stress_max_position_notional_usd {
        cmd.args(["--stress-max-position-notional-usd", &notional.to_string()]);
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            let reason = format!(
                "failed to spawn {python_executable} -m \
                 mqk_research.ml.p7a_p7b_economic_replay_stress_cli: {e}"
            );
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: None,
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(e) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = format!(
                "p7a_p7b_economic_replay_stress_cli produced unparseable output (exit={:?}): \
                 {e}; stdout={stdout:?} stderr={stderr:?}",
                output.status.code()
            );
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: None,
            };
        }
    };

    dispatch_cli_value(
        value,
        trial_id,
        economic_eval_id,
        expected_strategy_id,
        max_drawdown_ceiling,
        research_trial_id,
    )
    .unwrap_or_else(|e| e)
}

/// Pure dispatch of the parsed `mqk_research.ml.p7a_p7b_economic_replay_stress_cli`
/// JSON response into a [`RobustnessScenarioOutcome`] -- extracted from
/// [`p7a_p7b_economic_replay_stress_scenario`] so the mapping itself (in
/// particular, FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A's "MANDATORY
/// MEANS MANDATORY": `not_evaluable` must map to `applicable: true`, never
/// `applicable: false`) is directly unit-testable against a hand-built
/// `serde_json::Value`, with no subprocess required. Infallible (always
/// returns a `RobustnessScenarioOutcome`) -- the `Result`/`unwrap_or_else`
/// wrapper at the call site exists only so this function's every branch can
/// use ordinary `return`-free tail expressions.
fn dispatch_cli_value(
    value: serde_json::Value,
    trial_id: &str,
    economic_eval_id: &str,
    expected_strategy_id: &str,
    max_drawdown_ceiling: f64,
    research_trial_id: Option<String>,
) -> Result<RobustnessScenarioOutcome, RobustnessScenarioOutcome> {
    let name = P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string();

    // Cross-candidate authority -- checked BEFORE the status dispatch, same
    // discipline as `dsr_pbo_sensitivity_scenario`.
    if let Some(actual_strategy_id) = value.get("strategy_id").and_then(|v| v.as_str()) {
        if actual_strategy_id != expected_strategy_id {
            let reason = format!(
                "Research trial mismatch: trial_id {trial_id:?} is registered under \
                 strategy_id {actual_strategy_id:?}, but this backtest candidate is \
                 strategy_id {expected_strategy_id:?} -- refusing to merge P7A/P7B replay \
                 stress evidence from an unrelated Research trial"
            );
            return Err(RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: None,
            });
        }
    }

    // FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section B: the CLI's own resolved
    // `baseline_economic_eval_id` (when present -- only on "evaluated") must
    // agree with the caller-supplied `economic_eval_id` E. The Python CLI
    // already enforces this as its OWN binding authority; this is a cheap,
    // independent cross-check against caller/CLI drift, same discipline as
    // the `strategy_id` cross-check above.
    if let Some(actual_eval_id) = value.get("baseline_economic_eval_id").and_then(|v| v.as_str()) {
        if actual_eval_id != economic_eval_id {
            let reason = format!(
                "economic_eval_id mismatch: CLI resolved baseline_economic_eval_id \
                 {actual_eval_id:?}, but caller required {economic_eval_id:?} -- refusing to \
                 merge P7A/P7B replay stress evidence bound to a different economic result"
            );
            return Err(RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: Some(value),
            });
        }
    }

    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
    Ok(match status {
        "evaluated" => {
            let passed = value.get("passed").and_then(|v| v.as_bool());
            let stressed_max_drawdown = value.get("stressed_max_drawdown").and_then(|v| v.as_f64());
            match (passed, stressed_max_drawdown) {
                (Some(passed), Some(dd)) => RobustnessScenarioOutcome {
                    name,
                    applicable: true,
                    passed,
                    reason: if passed {
                        None
                    } else {
                        Some(format!(
                            "P7A/P7B stressed replay breached the conservative max-drawdown \
                             ceiling: stressed_max_drawdown={dd:.6} (ceiling \
                             -{max_drawdown_ceiling:.6}) -- reported as found, not tuned away"
                        ))
                    },
                    detail: format!(
                        "trial_id={trial_id}, stressed_max_drawdown={dd:.6} (ceiling \
                         -{max_drawdown_ceiling:.6}), stress_spec={:?}",
                        value.get("stress_spec")
                    ),
                    research_trial_id,
                    evidence: Some(value),
                },
                _ => {
                    let reason = format!("evaluated result missing passed/stressed_max_drawdown: {value}");
                    RobustnessScenarioOutcome {
                        name,
                        applicable: true,
                        passed: false,
                        reason: Some(reason.clone()),
                        detail: reason,
                        research_trial_id,
                        evidence: Some(value),
                    }
                }
            }
        }
        "not_evaluable" => {
            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            // FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A ("MANDATORY MEANS
            // MANDATORY"): a structural precondition this candidate's OWN
            // evidence does not meet (predates durable replay-input
            // recording, never engaged the official P7A/P7B protocols, lacks
            // the discrete-economics fold marker, or does not bind to the
            // required economic_eval_id) is a genuine FAIL for
            // promotion-grade P9 completeness -- it must NEVER disappear via
            // `applicable: false`. Distinct from a tamper/mismatch, which
            // the CLI always reports as `"status": "error"` (handled below).
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: Some(value),
            }
        }
        _ => {
            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            let full = format!("p7a_p7b_economic_replay_stress_cli error: {reason}");
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(full.clone()),
                detail: full,
                research_trial_id,
                evidence: Some(value),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P9-P7A-P7B-REAL-STRESS-01: an invalid `max_drawdown_ceiling` fails
    /// closed BEFORE any subprocess is spawned.
    #[test]
    fn negative_max_drawdown_ceiling_fails_closed_before_any_spawn() {
        let outcome = p7a_p7b_economic_replay_stress_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/stress_out"),
            20,
            50,
            None,
            None,
            -0.01,
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(
            outcome.reason.unwrap_or_default().contains("invalid max_drawdown_ceiling"),
            "must name the exact invalid parameter"
        );
    }

    #[test]
    fn max_drawdown_ceiling_above_one_fails_closed() {
        let outcome = p7a_p7b_economic_replay_stress_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/stress_out"),
            20,
            50,
            None,
            None,
            1.5,
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("invalid max_drawdown_ceiling"));
    }

    #[test]
    fn nan_max_drawdown_ceiling_fails_closed() {
        let outcome = p7a_p7b_economic_replay_stress_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/stress_out"),
            20,
            50,
            None,
            None,
            f64::NAN,
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("invalid max_drawdown_ceiling"));
    }

    /// Every rejected outcome (including invalid-threshold rejections) still
    /// carries `research_trial_id` -- PROMOTION-RESEARCH-BACKTEST-TRIAL-
    /// BINDING-01 requires this on every returned outcome.
    #[test]
    fn invalid_threshold_outcome_still_carries_research_trial_id() {
        let outcome = p7a_p7b_economic_replay_stress_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "trial_xyz",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/stress_out"),
            20,
            50,
            None,
            None,
            -1.0,
        );
        assert_eq!(outcome.research_trial_id.as_deref(), Some("trial_xyz"));
    }

    // -----------------------------------------------------------------
    // dispatch_cli_value: pure mapping tests (no subprocess required)
    // -----------------------------------------------------------------

    /// FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A ("MANDATORY MEANS
    /// MANDATORY"): a `not_evaluable` CLI response must map to
    /// `applicable: true, passed: false` -- it must NEVER disappear from a
    /// promotion-grade P9 artifact via `applicable: false`, unlike a
    /// genuinely optional scenario.
    #[test]
    fn not_evaluable_maps_to_applicable_true_passed_false_never_inapplicable() {
        let value = serde_json::json!({
            "status": "not_evaluable",
            "strategy_id": "some_strategy",
            "reason": "baseline execution_pricing.pricing_model_id is not the official model",
        });
        let outcome = dispatch_cli_value(
            value,
            "some_trial",
            "some_eval_id",
            "some_strategy",
            0.5,
            Some("some_trial".to_string()),
        )
        .unwrap_or_else(|e| e);
        assert!(outcome.applicable, "not_evaluable must never become applicable: false");
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("official model"));
    }

    /// Section B: the CLI's own resolved `baseline_economic_eval_id`
    /// disagreeing with the caller-required `economic_eval_id` fails closed
    /// -- same trial, different economic result is a mismatch.
    #[test]
    fn economic_eval_id_mismatch_fails_closed() {
        let value = serde_json::json!({
            "status": "evaluated",
            "strategy_id": "some_strategy",
            "baseline_economic_eval_id": "eval_id_actually_used",
            "passed": true,
            "stressed_max_drawdown": -0.01,
        });
        let outcome = dispatch_cli_value(
            value,
            "some_trial",
            "eval_id_caller_required",
            "some_strategy",
            0.5,
            Some("some_trial".to_string()),
        )
        .unwrap_or_else(|e| e);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("economic_eval_id mismatch"));
    }

    /// A genuine `strategy_id` mismatch still fails closed exactly as before
    /// this refactor (regression proof that extracting `dispatch_cli_value`
    /// preserved this existing invariant).
    #[test]
    fn strategy_id_mismatch_still_fails_closed() {
        let value = serde_json::json!({
            "status": "evaluated",
            "strategy_id": "other_strategy",
            "baseline_economic_eval_id": "e",
            "passed": true,
            "stressed_max_drawdown": -0.01,
        });
        let outcome = dispatch_cli_value(value, "some_trial", "e", "expected_strategy", 0.5, None)
            .unwrap_or_else(|e| e);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("Research trial mismatch"));
    }

    /// A genuine "evaluated, passed" response carries the full structured
    /// evidence blob durably (Section G) -- never reduced to a detail
    /// string only.
    #[test]
    fn evaluated_pass_carries_structured_evidence() {
        let value = serde_json::json!({
            "status": "evaluated",
            "strategy_id": "some_strategy",
            "baseline_economic_eval_id": "e",
            "stressed_economic_eval_id": "e2",
            "passed": true,
            "stressed_max_drawdown": -0.01,
            "bars_csv_sha256": "abc",
        });
        let outcome = dispatch_cli_value(
            value,
            "some_trial",
            "e",
            "some_strategy",
            0.5,
            Some("some_trial".to_string()),
        )
        .unwrap_or_else(|e| e);
        assert!(outcome.applicable);
        assert!(outcome.passed);
        let evidence = outcome.evidence.expect("evaluated outcome must carry structured evidence");
        assert_eq!(evidence.get("bars_csv_sha256").and_then(|v| v.as_str()), Some("abc"));
        assert_eq!(evidence.get("stressed_economic_eval_id").and_then(|v| v.as_str()), Some("e2"));
    }

    /// A drawdown-ceiling breach ("evaluated, not passed") is a genuine
    /// found failure, not tuned away -- and still carries evidence.
    #[test]
    fn evaluated_fail_reports_ceiling_breach_reason() {
        let value = serde_json::json!({
            "status": "evaluated",
            "strategy_id": "some_strategy",
            "baseline_economic_eval_id": "e",
            "passed": false,
            "stressed_max_drawdown": -0.9,
        });
        let outcome = dispatch_cli_value(value, "some_trial", "e", "some_strategy", 0.3, None)
            .unwrap_or_else(|e| e);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("breached the conservative max-drawdown"));
        assert!(outcome.evidence.is_some());
    }

    /// A genuine operational CLI error (`status: "error"`) fails closed,
    /// distinct from `not_evaluable`.
    #[test]
    fn error_status_fails_closed() {
        let value = serde_json::json!({"status": "error", "reason": "unknown trial_id"});
        let outcome = dispatch_cli_value(value, "some_trial", "e", "some_strategy", 0.3, None)
            .unwrap_or_else(|e| e);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("unknown trial_id"));
    }
}
