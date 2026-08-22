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
//! any missing/mutated input rather than refetching and assuming identity. A
//! trial whose economic evidence predates this durable-input recording, or
//! never engaged the OFFICIAL P7A/P7B protocols, is reported as genuinely
//! `applicable: false` here (mirrors `symbol_leave_one_out_scenario`'s own
//! "does not apply to this candidate" precedent) -- never fabricated as a
//! pass.
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

/// Run the genuine P7A/P7B economic replay stress for `trial_id` against
/// `registry_db`, re-evaluating that trial's FROZEN OOS prediction stream
/// (never re-trained) through the real `run_economic_walkforward` machinery
/// under an explicit stress configuration.
///
/// `python_executable`/`research_py_root` are supplied by the caller, same
/// contract as [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`].
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
/// `strategy_id` mismatch, or a genuine CLI error (bad registry, unknown
/// trial, missing/mutated replay input) all become
/// `applicable: true, passed: false` with the real reason. A baseline that
/// never engaged the official P7A/P7B protocols, has no durably-recorded
/// replay inputs, or lacks the discrete-economics fold marker is reported as
/// genuinely inapplicable (`applicable: false`) -- these are structural
/// preconditions this candidate's OWN evidence does not meet, never a
/// failure of the stress itself.
#[allow(clippy::too_many_arguments)]
pub fn p7a_p7b_economic_replay_stress_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
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
            };
        }
    };

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
            // Structural preconditions this candidate's OWN evidence does
            // not meet (predates durable replay-input recording, never
            // engaged the official P7A/P7B protocols, or lacks the
            // discrete-economics fold marker) -- a genuine "does not apply
            // to this candidate", never a failure of the stress itself.
            // Distinct from a tamper/mismatch, which the CLI always reports
            // as `"status": "error"` (handled below), never "not_evaluable".
            RobustnessScenarioOutcome {
                name,
                applicable: false,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id,
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
}
