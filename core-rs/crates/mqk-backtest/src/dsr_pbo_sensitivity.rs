//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` -- DSR/PBO sensitivity via cross-language
//! orchestration of the FROZEN, accepted Python multiple-testing judge.
//!
//! This module never re-implements Deflated Sharpe Ratio / Probability of
//! Backtest Overfitting statistics in Rust (forbidden -- see
//! `crate::robustness_gauntlet`'s own module docs and CLAUDE.md's
//! "statistical research" rule: once a method is verified and frozen, do
//! not re-derive it). It shells out to `research-py`'s
//! `mqk_research.ml.dsr_pbo_sensitivity_cli`, a thin wrapper that itself
//! only calls the existing, frozen `build_multiple_testing_judge` multiple
//! times under different `cscv_target_block_count` values -- a documented,
//! already-anticipated safe re-run (see that CLI's own module docs and
//! `ResearchResultStore.register_judge_artifact`'s docstring: distinct
//! block counts legitimately produce distinct, individually-durable,
//! individually-auditable judge artifacts sharing one `judge_id`; this is
//! not a new write path, nor registry pollution).
//!
//! Deliberately kept OUT of `crate::robustness_gauntlet::run_robustness_gauntlet`
//! itself (a pure, I/O-free-beyond-the-backtest-engine function) because
//! this scenario needs real subprocess + filesystem I/O. Callers assembling
//! the complete P9 evidence artifact call [`dsr_pbo_sensitivity_scenario`]
//! separately and merge it in via
//! `RobustnessGauntletOutput::merge_dsr_pbo_sensitivity`.

use std::path::Path;
use std::process::Command;

use crate::robustness_gauntlet::RobustnessScenarioOutcome;

/// Scenario name this module reports under -- part of
/// `robustness_gauntlet::REQUIRED_ROBUSTNESS_SCENARIO_NAMES`.
pub const DSR_PBO_SENSITIVITY_SCENARIO_NAME: &str = "dsr_pbo_sensitivity";

/// Default CSCV block-count grid perturbed across. Fixed and versioned as
/// part of `robustness_gauntlet::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION` --
/// changing it changes what evidence this scenario represents.
pub const DEFAULT_BLOCK_COUNTS: &[u32] = &[8, 10, 12];

/// Maximum allowed spread (max - min) in Deflated Sharpe Ratio across the
/// block-count grid before this scenario reports a genuine sensitivity
/// finding. Per the placebo scenario's own precedent: a candidate whose
/// DSR/PBO swings with an arbitrary CSCV partitioning choice is reported as
/// found, never tuned away by loosening this ceiling.
pub const DSR_MAX_SENSITIVITY_RANGE: f64 = 0.25;
/// Same discipline as [`DSR_MAX_SENSITIVITY_RANGE`], for PBO (a
/// probability, so this ceiling is itself bounded to `[0, 1]`).
pub const PBO_MAX_SENSITIVITY_RANGE: f64 = 0.25;

/// Run the DSR/PBO sensitivity scenario for `trial_id` against
/// `registry_db`, re-running the frozen judge once per entry in
/// `block_counts` (must be non-empty, each value even and >= 4 -- the same
/// constraint `JudgeSpec.cscv_target_block_count` already enforces
/// Python-side; a malformed grid surfaces as a `passed: false` scenario
/// with the Python CLI's own error, never a Rust panic).
///
/// `python_executable` and `research_py_root` (the directory containing
/// `research-py/src/mqk_research`) are supplied by the caller -- this
/// function performs zero environment-variable reads or path discovery of
/// its own, so it stays deterministic and testable; the caller is
/// responsible for resolving trusted application/config state exactly like
/// every other Research-registry-touching seam in this codebase
/// (`MQK_RESEARCH_REGISTRY_DB`, `MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT`).
///
/// `expected_strategy_id` is BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01's
/// cross-candidate authority check: the CLI resolves `trial_id`'s own
/// registered `strategy_id` from the registry and this function rejects a
/// mismatch against `expected_strategy_id` (the backtest candidate's own
/// `report.strategy_name`) BEFORE the result is ever merged into that
/// candidate's P9 evidence -- an operator supplying the wrong `--trial-id`
/// at finalization time is caught here, not only later at promotion time.
///
/// Fails closed: a spawn failure, unparseable output, a `strategy_id`
/// mismatch, or a genuine CLI error (bad registry, unknown trial) all
/// become `applicable: true, passed: false` with the real reason -- never
/// silently skipped. A structurally too-small comparison population
/// (mirroring the frozen judge's own `insufficient_candidates_for_cscv` /
/// `insufficient_trial_population_for_correction` reason codes) is the ONE
/// case reported as genuinely inapplicable (`applicable: false`) rather
/// than a failure, matching `symbol_leave_one_out_scenario`'s own
/// precedent for an honest "does not apply to this candidate."
pub fn dsr_pbo_sensitivity_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    expected_strategy_id: &str,
    block_counts: &[u32],
) -> RobustnessScenarioOutcome {
    let name = DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string();
    // PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: this scenario's evidence
    // is always computed against exactly `trial_id`, regardless of outcome
    // (evaluated, not_evaluable, or error) -- recorded on every returned
    // outcome so a later promotion decision can prove which Research trial
    // this P9 evidence came from, not merely which strategy_id.
    let research_trial_id = Some(trial_id.to_string());

    if block_counts.is_empty() {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some("block_counts must be non-empty".to_string()),
            detail: "block_counts must be non-empty".to_string(),
            research_trial_id,
        };
    }
    let block_counts_arg = block_counts
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let src_dir = research_py_root.join("src");

    let spawn_result = Command::new(python_executable)
        .env("PYTHONPATH", &src_dir)
        .args([
            "-m",
            "mqk_research.ml.dsr_pbo_sensitivity_cli",
            "--registry-db",
            &registry_db.display().to_string(),
            "--trial-id",
            trial_id,
            "--block-counts",
            &block_counts_arg,
        ])
        .output();

    let output = match spawn_result {
        Ok(o) => o,
        Err(e) => {
            let reason = format!(
                "failed to spawn {python_executable} -m mqk_research.ml.dsr_pbo_sensitivity_cli: {e}"
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
                "dsr_pbo_sensitivity_cli produced unparseable output (exit={:?}): {e}; \
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
            };
        }
    };

    // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: cross-candidate
    // authority -- checked BEFORE the status dispatch so a genuine
    // Research-trial mismatch is never masked by a more permissive
    // "evaluated"/"not_evaluable" reason. `strategy_id` is present on both
    // the "evaluated" and "not_evaluable" outcomes (see the CLI's own
    // module docs); only a genuine operational "error" (e.g. unknown
    // trial_id, caught by the trial-lookup failure itself) may lack it,
    // and that path already fails independently below.
    if let Some(actual_strategy_id) = value.get("strategy_id").and_then(|v| v.as_str()) {
        if actual_strategy_id != expected_strategy_id {
            let reason = format!(
                "Research trial mismatch: trial_id {trial_id:?} is registered under \
                 strategy_id {actual_strategy_id:?}, but this backtest candidate is \
                 strategy_id {expected_strategy_id:?} -- refusing to merge sensitivity \
                 evidence from an unrelated Research trial"
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
            let dsr_range = value.get("dsr_range").and_then(|v| v.as_f64());
            let pbo_range = value.get("pbo_range").and_then(|v| v.as_f64());
            match (dsr_range, pbo_range) {
                (Some(dr), Some(pr)) => {
                    let passed = dr <= DSR_MAX_SENSITIVITY_RANGE && pr <= PBO_MAX_SENSITIVITY_RANGE;
                    RobustnessScenarioOutcome {
                        name,
                        applicable: true,
                        passed,
                        reason: if passed {
                            None
                        } else {
                            Some(format!(
                                "DSR/PBO too sensitive to CSCV block-count choice: dsr_range={dr:.6} \
                                 (ceiling {DSR_MAX_SENSITIVITY_RANGE}), pbo_range={pr:.6} \
                                 (ceiling {PBO_MAX_SENSITIVITY_RANGE}) -- reported as found, not \
                                 tuned away"
                            ))
                        },
                        detail: format!(
                            "block_counts={block_counts:?}, dsr_range={dr:.6}, pbo_range={pr:.6}"
                        ),
                        research_trial_id,
                    }
                }
                _ => {
                    let reason = format!("evaluated result missing dsr_range/pbo_range: {value}");
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
            let genuinely_inapplicable = reason.contains("insufficient_candidates_for_cscv")
                || reason.contains("insufficient_trial_population_for_correction")
                || reason.contains("not part of the judged comparison scope");
            RobustnessScenarioOutcome {
                name,
                applicable: !genuinely_inapplicable,
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
            let full = format!("dsr_pbo_sensitivity_cli error: {reason}");
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
