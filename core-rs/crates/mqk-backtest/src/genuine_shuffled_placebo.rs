//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` / FINAL-P9-ROBUSTNESS-SEMANTICS-01 --
//! genuine shuffled/null-control placebo, via cross-language orchestration
//! of the FROZEN, accepted Python economic-walkforward engine.
//!
//! Supersedes `robustness_gauntlet::placebo_temporal_offset_scenario` as the
//! REQUIRED placebo evidence: a temporal-delay placebo still trades each
//! decision's own real score, merely against a different bar -- it is not a
//! shuffled/random-label placebo. This module shells out to `research-py`'s
//! `mqk_research.ml.genuine_shuffled_placebo_cli`, which resolves an
//! ALREADY-REGISTERED Research trial's EXACT succeeded attempt (same
//! `economic_eval_id`-binding and durable-hash authentication as
//! [`crate::p7a_p7b_economic_replay_stress`]), deterministically permutes
//! its frozen OOS `ml_score` stream WITHIN each fold (same marginal
//! distribution of scores, decoupled from `(symbol, decision_ts)`), and
//! re-runs the real `run_economic_walkforward` against it -- never a new
//! model, never a new Research trial.
//!
//! Deliberately kept OUT of
//! `crate::robustness_gauntlet::run_robustness_gauntlet` itself, exactly
//! like [`crate::p7a_p7b_economic_replay_stress::p7a_p7b_economic_replay_stress_scenario`]
//! and [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`] -- this
//! scenario needs real subprocess + filesystem I/O and a completed Research
//! trial.

use std::path::Path;
use std::process::Command;

use crate::robustness_gauntlet::RobustnessScenarioOutcome;

/// Scenario name this module reports under -- part of
/// `robustness_gauntlet::REQUIRED_ROBUSTNESS_SCENARIO_NAMES`.
pub const GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME: &str = "genuine_shuffled_placebo";

/// Run the genuine shuffled placebo control for `trial_id`/`economic_eval_id`
/// against `registry_db`. Same caller contract as
/// [`crate::p7a_p7b_economic_replay_stress::p7a_p7b_economic_replay_stress_scenario`]:
/// `python_executable`/`research_py_root` are supplied by the caller;
/// `economic_eval_id` is the EXACT, REQUIRED P7C-authorized economic result
/// this control must bind to; `expected_strategy_id` is the same
/// cross-candidate authority check.
///
/// FINAL-P9-ROBUSTNESS-SEMANTICS-01 ("MANDATORY MEANS MANDATORY", mirroring
/// FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A): fails closed --
/// `not_evaluable` (missing durable replay inputs, too few OOS rows to
/// shuffle meaningfully) maps to `applicable: true, passed: false`, never
/// `applicable: false`. A spawn failure, unparseable output, a
/// `strategy_id`/`economic_eval_id` mismatch, or a genuine CLI error all
/// become `applicable: true, passed: false` with the real reason.
pub fn genuine_shuffled_placebo_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    expected_strategy_id: &str,
    placebo_out_dir: &Path,
) -> RobustnessScenarioOutcome {
    let name = GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME.to_string();
    let research_trial_id = Some(trial_id.to_string());

    let src_dir = research_py_root.join("src");
    let cmd_output = Command::new(python_executable)
        .env("PYTHONPATH", &src_dir)
        .args([
            "-m",
            "mqk_research.ml.genuine_shuffled_placebo_cli",
            "--registry-db",
            &registry_db.display().to_string(),
            "--trial-id",
            trial_id,
            "--economic-eval-id",
            economic_eval_id,
            "--placebo-out-dir",
            &placebo_out_dir.display().to_string(),
        ])
        .output();

    let output = match cmd_output {
        Ok(o) => o,
        Err(e) => {
            let reason = format!(
                "failed to spawn {python_executable} -m mqk_research.ml.genuine_shuffled_placebo_cli: {e}"
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
                "genuine_shuffled_placebo_cli produced unparseable output (exit={:?}): {e}; \
                 stdout={stdout:?} stderr={stderr:?}",
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

    if let Some(actual_strategy_id) = value.get("strategy_id").and_then(|v| v.as_str()) {
        if actual_strategy_id != expected_strategy_id {
            let reason = format!(
                "Research trial mismatch: trial_id {trial_id:?} is registered under \
                 strategy_id {actual_strategy_id:?}, but this backtest candidate is \
                 strategy_id {expected_strategy_id:?} -- refusing to merge genuine shuffled \
                 placebo evidence from an unrelated Research trial"
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
    if let Some(actual_eval_id) = value.get("baseline_economic_eval_id").and_then(|v| v.as_str()) {
        if actual_eval_id != economic_eval_id {
            let reason = format!(
                "economic_eval_id mismatch: CLI resolved baseline_economic_eval_id \
                 {actual_eval_id:?}, but caller required {economic_eval_id:?} -- refusing to \
                 merge genuine shuffled placebo evidence bound to a different economic result"
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
            let baseline = value.get("baseline_net_total_return").and_then(|v| v.as_f64());
            let placebo = value.get("placebo_net_total_return").and_then(|v| v.as_f64());
            match (passed, baseline, placebo) {
                (Some(passed), Some(b), Some(p)) => RobustnessScenarioOutcome {
                    name,
                    applicable: true,
                    passed,
                    reason: if passed {
                        None
                    } else {
                        Some(format!(
                            "genuine shuffled placebo performed as well or better than the real \
                             signal: baseline_net_total_return={b:.6}, \
                             placebo_net_total_return={p:.6} -- this candidate's real signal may \
                             not be distinguishable from noise; reported as found, not tuned away"
                        ))
                    },
                    detail: format!(
                        "trial_id={trial_id}, baseline_net_total_return={b:.6}, \
                         placebo_net_total_return={p:.6}"
                    ),
                    research_trial_id,
                    evidence: Some(value),
                },
                _ => {
                    let reason = format!(
                        "evaluated result missing passed/baseline_net_total_return/\
                         placebo_net_total_return: {value}"
                    );
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
            // MANDATORY MEANS MANDATORY: never applicable: false.
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
            let full = format!("genuine_shuffled_placebo_cli error: {reason}");
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

    #[test]
    fn spawn_failure_fails_closed_applicable_true() {
        let outcome = genuine_shuffled_placebo_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_eval_id",
            "some_strategy",
            Path::new("/nonexistent/placebo_out"),
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("failed to spawn"));
        assert_eq!(outcome.research_trial_id.as_deref(), Some("some_trial"));
    }
}
