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

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::robustness_gauntlet::RobustnessScenarioOutcome;

/// Half-exposure stress as a RECOMPUTED capital-fraction target quantity.
///
/// `signals_csv` / `signals_meta` are a native signal stream of the SAME registered trial that
/// the Rust emitter produced under `FixedInitialCapitalFractionV1` at `allocation_fraction_bps`
/// (strictly below the baseline's) on the same immutable capital and bars;
/// `expected_semantic_fingerprint` is the wrapper fingerprint `native-fingerprint` resolves for
/// that fraction. The Python replay authenticates the stream against the baseline's, so the
/// decisions are provably unchanged and only the quantity differs. It is never a USD cap on the
/// baseline quantity, which the exact-target replay refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapitalFractionStressSizing {
    pub scenario_id: String,
    pub allocation_fraction_bps: i64,
    pub signals_csv: PathBuf,
    pub signals_meta: PathBuf,
    pub expected_semantic_fingerprint: String,
}

impl CapitalFractionStressSizing {
    /// Fail-closed shape validation, run BEFORE any subprocess is spawned.
    fn validate(&self) -> Result<(), String> {
        if self.scenario_id.trim().is_empty() {
            return Err("capital-fraction stress scenario_id is empty".to_string());
        }
        if !(1..=10_000).contains(&self.allocation_fraction_bps) {
            return Err(format!(
                "capital-fraction stress allocation_fraction_bps {} is outside 1..=10000",
                self.allocation_fraction_bps
            ));
        }
        let fp = &self.expected_semantic_fingerprint;
        if fp.len() != 64
            || !fp
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(
                "capital-fraction stress expected_semantic_fingerprint is not 64 lowercase hex"
                    .to_string(),
            );
        }
        Ok(())
    }
}

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
    p7a_p7b_stress_scenario_impl(
        python_executable,
        research_py_root,
        registry_db,
        trial_id,
        economic_eval_id,
        expected_strategy_id,
        stress_out_dir,
        stress_execution_slippage_bps,
        stress_execution_volatility_mult_bps,
        stress_max_target_qty,
        stress_max_position_notional_usd,
        max_drawdown_ceiling,
        None,
    )
}

/// The half-exposure variant of [`p7a_p7b_economic_replay_stress_scenario`]: P7B is expressed as a
/// recomputed capital-fraction quantity (`sizing`), never as a post-sizing cap, so no
/// `stress_max_*` knob exists on this entry point and the baseline safety caps are carried
/// unchanged. Same replay authority, same strategy/eval-id cross-checks, same
/// "mandatory means mandatory" outcome mapping; the returned evidence additionally has to echo the
/// requested scenario and fraction.
#[allow(clippy::too_many_arguments)]
pub fn p7a_p7b_capital_fraction_stress_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    expected_strategy_id: &str,
    stress_out_dir: &Path,
    stress_execution_slippage_bps: u32,
    stress_execution_volatility_mult_bps: u32,
    max_drawdown_ceiling: f64,
    sizing: &CapitalFractionStressSizing,
) -> RobustnessScenarioOutcome {
    let outcome = p7a_p7b_stress_scenario_impl(
        python_executable,
        research_py_root,
        registry_db,
        trial_id,
        economic_eval_id,
        expected_strategy_id,
        stress_out_dir,
        stress_execution_slippage_bps,
        stress_execution_volatility_mult_bps,
        None,
        None,
        max_drawdown_ceiling,
        Some(sizing),
    );
    require_stress_sizing_echo(outcome, sizing)
}

/// An `evaluated` capital-fraction stress is only accepted when its evidence names the requested
/// scenario and fraction; anything else is a genuine FAIL, never a silent pass.
fn require_stress_sizing_echo(
    mut outcome: RobustnessScenarioOutcome,
    sizing: &CapitalFractionStressSizing,
) -> RobustnessScenarioOutcome {
    let Some(evidence) = outcome.evidence.as_ref() else {
        return outcome;
    };
    if evidence.get("status").and_then(|v| v.as_str()) != Some("evaluated") {
        return outcome;
    }
    let echoed = evidence
        .get("stress_spec")
        .and_then(|s| s.get("stress_sizing"));
    let ok = echoed.is_some_and(|e| {
        e.get("allocation_fraction_bps").and_then(|v| v.as_i64())
            == Some(sizing.allocation_fraction_bps)
            && e.get("scenario_id").and_then(|v| v.as_str()) == Some(sizing.scenario_id.as_str())
            && e.get("is_a_trial").and_then(|v| v.as_bool()) == Some(false)
    });
    if !ok {
        let reason = format!(
            "stress evidence does not echo the requested capital-fraction scenario \
             {:?} at {} bps: {echoed:?}",
            sizing.scenario_id, sizing.allocation_fraction_bps
        );
        outcome.passed = false;
        outcome.reason = Some(reason.clone());
        outcome.detail = reason;
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn p7a_p7b_stress_scenario_impl(
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
    stress_sizing: Option<&CapitalFractionStressSizing>,
) -> RobustnessScenarioOutcome {
    let name = P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string();
    let research_trial_id = Some(trial_id.to_string());

    // A capital-fraction stress is never expressed as a cap, and its own shape is validated
    // BEFORE any I/O.
    if let Some(sizing) = stress_sizing {
        let defect = sizing.validate().err().or_else(|| {
            (stress_max_target_qty.is_some() || stress_max_position_notional_usd.is_some())
                .then(|| "capital-fraction stress forbids stress_max_target_qty / stress_max_position_notional_usd".to_string())
        });
        if let Some(reason) = defect {
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
    }

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
    if let Some(sizing) = stress_sizing {
        cmd.args([
            "--stress-allocation-fraction-bps",
            &sizing.allocation_fraction_bps.to_string(),
            "--stress-sizing-scenario-id",
            &sizing.scenario_id,
            "--stress-sizing-signals-csv",
            &sizing.signals_csv.display().to_string(),
            "--stress-sizing-signals-meta",
            &sizing.signals_meta.display().to_string(),
            "--stress-sizing-expected-semantic-fingerprint",
            &sizing.expected_semantic_fingerprint,
        ]);
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
}

/// Pure dispatch of the parsed `mqk_research.ml.p7a_p7b_economic_replay_stress_cli`
/// JSON response into a [`RobustnessScenarioOutcome`] -- extracted from
/// [`p7a_p7b_economic_replay_stress_scenario`] so the mapping itself (in
/// particular, FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A's "MANDATORY
/// MEANS MANDATORY": `not_evaluable` must map to `applicable: true`, never
/// `applicable: false`) is directly unit-testable against a hand-built
/// `serde_json::Value`, with no subprocess required. Infallible: every
/// branch returns a [`RobustnessScenarioOutcome`] directly, including
/// fail-closed identity/economic-evaluation mismatches.
fn dispatch_cli_value(
    value: serde_json::Value,
    trial_id: &str,
    economic_eval_id: &str,
    expected_strategy_id: &str,
    max_drawdown_ceiling: f64,
    research_trial_id: Option<String>,
) -> RobustnessScenarioOutcome {
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
    }

    // FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section B: the CLI's own resolved
    // `baseline_economic_eval_id` (when present -- only on "evaluated") must
    // agree with the caller-supplied `economic_eval_id` E. The Python CLI
    // already enforces this as its OWN binding authority; this is a cheap,
    // independent cross-check against caller/CLI drift, same discipline as
    // the `strategy_id` cross-check above.
    if let Some(actual_eval_id) = value
        .get("baseline_economic_eval_id")
        .and_then(|v| v.as_str())
    {
        if actual_eval_id != economic_eval_id {
            let reason = format!(
                "economic_eval_id mismatch: CLI resolved baseline_economic_eval_id \
                 {actual_eval_id:?}, but caller required {economic_eval_id:?} -- refusing to \
                 merge P7A/P7B replay stress evidence bound to a different economic result"
            );
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
                evidence: Some(value),
            };
        }
    }

    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");

    match status {
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
                    let reason =
                        format!("evaluated result missing passed/stressed_max_drawdown: {value}");

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
    }
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
            outcome
                .reason
                .unwrap_or_default()
                .contains("invalid max_drawdown_ceiling"),
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
        assert!(outcome
            .reason
            .unwrap_or_default()
            .contains("invalid max_drawdown_ceiling"));
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
        assert!(outcome
            .reason
            .unwrap_or_default()
            .contains("invalid max_drawdown_ceiling"));
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
        );
        assert!(
            outcome.applicable,
            "not_evaluable must never become applicable: false"
        );
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
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome
            .reason
            .unwrap()
            .contains("economic_eval_id mismatch"));
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
        let outcome = dispatch_cli_value(value, "some_trial", "e", "expected_strategy", 0.5, None);
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
        );
        assert!(outcome.applicable);
        assert!(outcome.passed);
        let evidence = outcome
            .evidence
            .expect("evaluated outcome must carry structured evidence");
        assert_eq!(
            evidence.get("bars_csv_sha256").and_then(|v| v.as_str()),
            Some("abc")
        );
        assert_eq!(
            evidence
                .get("stressed_economic_eval_id")
                .and_then(|v| v.as_str()),
            Some("e2")
        );
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
        let outcome = dispatch_cli_value(value, "some_trial", "e", "some_strategy", 0.3, None);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome
            .reason
            .unwrap()
            .contains("breached the conservative max-drawdown"));
        assert!(outcome.evidence.is_some());
    }

    /// A genuine operational CLI error (`status: "error"`) fails closed,
    /// distinct from `not_evaluable`.
    #[test]
    fn error_status_fails_closed() {
        let value = serde_json::json!({"status": "error", "reason": "unknown trial_id"});
        let outcome = dispatch_cli_value(value, "some_trial", "e", "some_strategy", 0.3, None);
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap().contains("unknown trial_id"));
    }
}

#[cfg(test)]
mod capital_fraction_stress_tests {
    use super::*;

    fn sizing(bps: i64) -> CapitalFractionStressSizing {
        CapitalFractionStressSizing {
            scenario_id: "half_exposure_capital_fraction_500bps_v1".to_string(),
            allocation_fraction_bps: bps,
            signals_csv: PathBuf::from("/nonexistent/stress_signals.csv"),
            signals_meta: PathBuf::from("/nonexistent/stress_meta.json"),
            expected_semantic_fingerprint: "a".repeat(64),
        }
    }

    fn run(sizing: &CapitalFractionStressSizing) -> RobustnessScenarioOutcome {
        p7a_p7b_capital_fraction_stress_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/stress_out"),
            15,
            10,
            0.40,
            sizing,
        )
    }

    fn assert_refused_before_spawn(outcome: &RobustnessScenarioOutcome, needle: &str) {
        assert!(
            outcome.applicable && !outcome.passed,
            "mandatory scenario must FAIL, not vanish"
        );
        let reason = outcome.reason.clone().unwrap_or_default();
        assert!(reason.contains(needle), "{reason}");
        assert!(
            !reason.contains("failed to spawn"),
            "must be refused BEFORE any spawn: {reason}"
        );
        assert!(outcome.evidence.is_none());
    }

    #[test]
    fn malformed_sizing_is_refused_before_any_subprocess() {
        for bps in [0, -1, 10_001] {
            assert_refused_before_spawn(&run(&sizing(bps)), "allocation_fraction_bps");
        }
        let mut s = sizing(500);
        s.expected_semantic_fingerprint = "A".repeat(64);
        assert_refused_before_spawn(&run(&s), "64 lowercase hex");
        s.expected_semantic_fingerprint = "a".repeat(63);
        assert_refused_before_spawn(&run(&s), "64 lowercase hex");
        let mut s = sizing(500);
        s.scenario_id = "  ".to_string();
        assert_refused_before_spawn(&run(&s), "scenario_id");
    }

    #[test]
    fn a_valid_sizing_reaches_the_spawn_stage_so_the_refusals_above_are_the_validator() {
        let outcome = run(&sizing(500));
        let reason = outcome.reason.unwrap_or_default();
        assert!(reason.contains("failed to spawn"), "{reason}");
    }

    #[test]
    fn caps_alongside_a_capital_fraction_stress_are_refused_before_any_subprocess() {
        for (qty, notional) in [
            (Some(30), None),
            (None, Some(5000.0)),
            (None, Some(25000.0)),
        ] {
            let outcome = p7a_p7b_stress_scenario_impl(
                "mqk_this_executable_must_never_be_invoked",
                Path::new("/nonexistent/research-py"),
                Path::new("/nonexistent/registry.sqlite3"),
                "t",
                "e",
                "s",
                Path::new("/nonexistent/stress_out"),
                15,
                10,
                qty,
                notional,
                0.40,
                Some(&sizing(500)),
            );
            assert_refused_before_spawn(&outcome, "forbids stress_max_target_qty");
        }
    }

    fn evaluated(stress_sizing: serde_json::Value) -> RobustnessScenarioOutcome {
        let evidence = serde_json::json!({
            "status": "evaluated",
            "stress_spec": { "stress_sizing": stress_sizing },
        });
        RobustnessScenarioOutcome {
            name: P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string(),
            applicable: true,
            passed: true,
            reason: None,
            detail: String::new(),
            research_trial_id: Some("t".to_string()),
            evidence: Some(evidence),
        }
    }

    #[test]
    fn evaluated_evidence_must_echo_the_requested_scenario_and_fraction() {
        let s = sizing(500);
        let good = serde_json::json!({
            "scenario_id": s.scenario_id, "allocation_fraction_bps": 500, "is_a_trial": false });
        assert!(require_stress_sizing_echo(evaluated(good.clone()), &s).passed);

        // A different fraction (501), a different scenario name, a trial flag or a cap-style
        // evidence block with no stress_sizing cannot satisfy the requested scenario.
        for bad in [
            serde_json::json!({ "scenario_id": s.scenario_id, "allocation_fraction_bps": 501, "is_a_trial": false }),
            serde_json::json!({ "scenario_id": "other", "allocation_fraction_bps": 500, "is_a_trial": false }),
            serde_json::json!({ "scenario_id": s.scenario_id, "allocation_fraction_bps": 500, "is_a_trial": true }),
            serde_json::Value::Null,
        ] {
            let out = require_stress_sizing_echo(evaluated(bad), &s);
            assert!(out.applicable && !out.passed, "{:?}", out.reason);
            assert!(out.reason.unwrap().contains("does not echo"));
        }
        // A non-evaluated outcome is returned untouched (its own failure reason stands).
        let mut failed = evaluated(good);
        failed.passed = false;
        failed.evidence = Some(serde_json::json!({"status": "error"}));
        failed.reason = Some("boom".to_string());
        assert_eq!(
            require_stress_sizing_echo(failed, &s).reason.as_deref(),
            Some("boom")
        );
    }
}
