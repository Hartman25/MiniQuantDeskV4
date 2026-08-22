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

// P9-P7A-P7B-REAL-STRESS-01: this module previously baked in a CSCV
// block-count default (`[8, 10, 12]`) and DSR/PBO sensitivity acceptance
// ceilings (`0.25`/`0.25`) as `pub const`s with no accepted Research/
// promotion-policy source establishing those specific numbers -- an
// independent review correctly flagged them as hidden, unauthorized policy.
// `block_counts` was already a required caller-supplied parameter (see
// below); `dsr_max_sensitivity_range`/`pbo_max_sensitivity_range` are now
// ALSO required parameters -- there is no in-crate default to fall back to,
// so a caller (the `mqk-cli` `FinalizeRobustnessSensitivity` command) must
// supply an explicit value or the CLI itself fails closed (no
// `default_value`, see `mqk-cli/src/main.rs`). This module still never
// decides WHAT that policy value should be -- only that it must be named
// explicitly, every time, by whoever finalizes P9 evidence for a candidate.

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
/// `authoritative_judge_artifact_sha256` (FINAL-P9-AUTHORITY-BINDING-REPAIR-01
/// Section 1) is the EXACT, already-registered P7C-authorized
/// `research_judge_artifacts.judge_artifact_sha256` whose registered
/// `(experiment_id, hypothesis_id)` scope this sensitivity sweep must reuse
/// -- passed straight through to the Python CLI, which resolves that exact
/// scope (never `trial_id`'s own registered `hypothesis_id`) and reuses it
/// for EVERY entry in `block_counts`. Required, no default: an empty value
/// is a genuine misconfiguration, reported `applicable: true, passed: false`
/// before any subprocess is spawned.
///
/// `dsr_max_sensitivity_range`/`pbo_max_sensitivity_range` are the
/// EXPLICIT, caller-supplied acceptance ceilings for the DSR/PBO spread
/// across `block_counts` (P9-P7A-P7B-REAL-STRESS-01: no hidden in-crate
/// default -- see module docs). `pbo_max_sensitivity_range` must lie in
/// `[0, 1]` (PBO is a probability); `dsr_max_sensitivity_range` must be
/// finite and `>= 0.0`. An out-of-range value is reported as a genuine
/// `applicable: true, passed: false` misconfiguration -- never silently
/// clamped or defaulted -- before any subprocess is spawned.
///
/// Fails closed: an invalid threshold/judge identity, a spawn failure,
/// unparseable output, a `strategy_id`/judge-scope mismatch, or a genuine
/// CLI error (bad registry, unknown trial, unknown judge, judge scope not
/// covering this trial) all become `applicable: true, passed: false` with
/// the real reason -- never silently skipped. FINAL-P9-AUTHORITY-BINDING-
/// REPAIR-01 Section 2 ("REQUIRED DSR/PBO MAY NEVER VANISH via
/// `applicable=false`"): this scenario is a REQUIRED promotion-grade P9
/// slice and NEVER reports `applicable: false` -- a structurally too-small
/// comparison population is a genuine FAIL (`applicable: true, passed:
/// false`), not an inapplicable exemption, matching
/// `p7a_p7b_economic_replay_stress_scenario`'s / `genuine_shuffled_placebo_
/// scenario`'s own "MANDATORY MEANS MANDATORY" discipline.
#[allow(clippy::too_many_arguments)]
pub fn dsr_pbo_sensitivity_scenario(
    python_executable: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    expected_strategy_id: &str,
    authoritative_judge_artifact_sha256: &str,
    block_counts: &[u32],
    dsr_max_sensitivity_range: f64,
    pbo_max_sensitivity_range: f64,
) -> RobustnessScenarioOutcome {
    let name = DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string();
    // PROMOTION-STRESS-AUTHORITY-REPAIR-01 / P9-P7A-P7B-REAL-STRESS-01:
    // validate the caller-supplied policy thresholds BEFORE any I/O -- an
    // invalid ceiling is a real misconfiguration, never silently accepted.
    if !dsr_max_sensitivity_range.is_finite() || dsr_max_sensitivity_range < 0.0 {
        let reason = format!(
            "invalid dsr_max_sensitivity_range policy value: {dsr_max_sensitivity_range} \
             (must be finite and >= 0.0)"
        );
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(reason.clone()),
            detail: reason,
            research_trial_id: Some(trial_id.to_string()),
            evidence: None,
        };
    }
    if !pbo_max_sensitivity_range.is_finite() || !(0.0..=1.0).contains(&pbo_max_sensitivity_range) {
        let reason = format!(
            "invalid pbo_max_sensitivity_range policy value: {pbo_max_sensitivity_range} \
             (must be finite and within [0, 1] -- PBO is a probability)"
        );
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(reason.clone()),
            detail: reason,
            research_trial_id: Some(trial_id.to_string()),
            evidence: None,
        };
    }
    // PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: this scenario's evidence
    // is always computed against exactly `trial_id`, regardless of outcome
    // (evaluated, not_evaluable, or error) -- recorded on every returned
    // outcome so a later promotion decision can prove which Research trial
    // this P9 evidence came from, not merely which strategy_id.
    let research_trial_id = Some(trial_id.to_string());

    // FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: an empty judge
    // identity is a genuine misconfiguration -- fail closed before any
    // subprocess is spawned, mirroring the threshold checks above.
    if authoritative_judge_artifact_sha256.trim().is_empty() {
        let reason = "authoritative_judge_artifact_sha256 must not be empty -- the EXACT \
                       P7C-authorized judge scope this sensitivity sweep must reuse is required"
            .to_string();
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

    if block_counts.is_empty() {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some("block_counts must be non-empty".to_string()),
            detail: "block_counts must be non-empty".to_string(),
            research_trial_id,
            evidence: None,
        };
    }
    // FINAL-P9-ROBUSTNESS-SEMANTICS-01 Section A: a sensitivity grid with
    // fewer than 2 DISTINCT block counts (`[8]`, or `[8, 8]`) cannot measure
    // sensitivity at all -- every re-run would trivially share the same (or
    // an unvaried) result, producing dsr_range/pbo_range == 0 by
    // construction rather than genuine evidence of insensitivity. This is a
    // structural requirement failure, not a not_evaluable/inapplicable
    // candidate property -- it fails BEFORE any subprocess is spawned.
    let distinct_block_counts: std::collections::BTreeSet<u32> = block_counts.iter().copied().collect();
    if distinct_block_counts.len() < 2 {
        let reason = format!(
            "block_counts must contain at least 2 DISTINCT values to measure sensitivity \
             (got {block_counts:?}, {} distinct) -- a single-value or duplicate-only grid \
             trivially reports zero sensitivity by construction, never genuine evidence",
            distinct_block_counts.len()
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
            "--judge-artifact-sha256",
            authoritative_judge_artifact_sha256,
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
                evidence: None,
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
                evidence: Some(value),
            };
        }
    }

    // FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: cheap, independent
    // cross-check that the CLI's own resolved
    // `authoritative_judge_artifact_sha256` (present whenever the CLI
    // reached a real scope resolution -- "evaluated" or "not_evaluable")
    // agrees with what THIS caller supplied -- same discipline as the
    // `strategy_id`/`baseline_economic_eval_id` cross-checks elsewhere in
    // this P9 module family, guarding against caller/CLI drift.
    if let Some(actual_judge_sha256) =
        value.get("authoritative_judge_artifact_sha256").and_then(|v| v.as_str())
    {
        if actual_judge_sha256 != authoritative_judge_artifact_sha256 {
            let reason = format!(
                "judge scope identity mismatch: CLI resolved \
                 authoritative_judge_artifact_sha256 {actual_judge_sha256:?}, but caller \
                 required {authoritative_judge_artifact_sha256:?} -- refusing to merge \
                 sensitivity evidence computed against a different judge scope"
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
            let dsr_range = value.get("dsr_range").and_then(|v| v.as_f64());
            let pbo_range = value.get("pbo_range").and_then(|v| v.as_f64());
            match (dsr_range, pbo_range) {
                (Some(dr), Some(pr)) => {
                    let passed =
                        dr <= dsr_max_sensitivity_range && pr <= pbo_max_sensitivity_range;
                    RobustnessScenarioOutcome {
                        name,
                        applicable: true,
                        passed,
                        reason: if passed {
                            None
                        } else {
                            Some(format!(
                                "DSR/PBO too sensitive to CSCV block-count choice: dsr_range={dr:.6} \
                                 (ceiling {dsr_max_sensitivity_range}), pbo_range={pr:.6} \
                                 (ceiling {pbo_max_sensitivity_range}) -- reported as found, not \
                                 tuned away"
                            ))
                        },
                        detail: format!(
                            "block_counts={block_counts:?}, dsr_range={dr:.6} \
                             (ceiling {dsr_max_sensitivity_range}), pbo_range={pr:.6} \
                             (ceiling {pbo_max_sensitivity_range})"
                        ),
                        research_trial_id,
                        evidence: Some(value),
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
            // FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 2 ("REQUIRED
            // DSR/PBO MAY NEVER VANISH via applicable=false"):
            // dsr_pbo_sensitivity is a REQUIRED promotion-grade P9 slice --
            // ANY not_evaluable outcome (insufficient candidate/trial
            // population, trial absent from comparison scope, or any other
            // inability to perform the requested perturbation) is a genuine
            // FAIL, never a "does not apply to this candidate" exemption --
            // it must NEVER disappear via `applicable: false`. Mirrors
            // p7a_p7b_economic_replay_stress_scenario's / genuine_shuffled_
            // placebo_scenario's own "MANDATORY MEANS MANDATORY" discipline.
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
            let full = format!("dsr_pbo_sensitivity_cli error: {reason}");
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

    /// FINAL-P9-ROBUSTNESS-SEMANTICS-01: a single-value block_counts grid
    /// fails closed before any subprocess is spawned.
    #[test]
    fn single_value_block_counts_fails_closed_before_any_spawn() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8],
            0.25,
            0.25,
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("2 DISTINCT values"));
    }

    /// A duplicate-only grid (`[8, 8]`) is exactly as unevaluable as a
    /// single value -- 2 raw entries but 1 distinct value.
    #[test]
    fn duplicate_only_block_counts_fails_closed() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 8],
            0.25,
            0.25,
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("2 DISTINCT values"));
    }

    /// A structurally valid, genuinely distinct 2-value grid must NOT be
    /// rejected by this check (it fails later, at spawn, for an unrelated
    /// reason -- proving this check doesn't over-reject).
    #[test]
    fn two_distinct_block_counts_passes_the_distinctness_check() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            0.25,
            0.25,
        );
        assert!(
            !outcome.reason.unwrap_or_default().contains("DISTINCT"),
            "a genuinely distinct grid must not be rejected for distinctness"
        );
    }

    /// P9-P7A-P7B-REAL-STRESS-01: an invalid `dsr_max_sensitivity_range`
    /// fails closed BEFORE any subprocess is spawned -- proven here without
    /// a real python (unlike `scenario_dsr_pbo_sensitivity_01.rs`'s
    /// `#[ignore]`d integration tests) since the invalid-python-executable
    /// path is never reached.
    #[test]
    fn negative_dsr_max_sensitivity_range_fails_closed_before_any_spawn() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            -0.01,
            0.25,
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(
            outcome.reason.unwrap_or_default().contains("invalid dsr_max_sensitivity_range"),
            "must name the exact invalid parameter"
        );
    }

    #[test]
    fn nan_dsr_max_sensitivity_range_fails_closed() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            f64::NAN,
            0.25,
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("invalid dsr_max_sensitivity_range"));
    }

    #[test]
    fn pbo_max_sensitivity_range_above_one_fails_closed() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            0.25,
            1.5, // PBO is a probability -- must be within [0, 1]
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("invalid pbo_max_sensitivity_range"));
    }

    #[test]
    fn negative_pbo_max_sensitivity_range_fails_closed() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            0.25,
            -0.01,
        );
        assert!(!outcome.passed);
        assert!(outcome.reason.unwrap_or_default().contains("invalid pbo_max_sensitivity_range"));
    }

    /// Every rejected outcome (including invalid-threshold rejections) still
    /// carries `research_trial_id` -- PROMOTION-RESEARCH-BACKTEST-TRIAL-
    /// BINDING-01 requires this on every returned outcome, not only the
    /// evaluated/not_evaluable ones.
    #[test]
    fn invalid_threshold_outcome_still_carries_research_trial_id() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "trial_xyz",
            "some_strategy",
            "some_judge_sha256",
            &[8, 10],
            -1.0,
            0.25,
        );
        assert_eq!(outcome.research_trial_id.as_deref(), Some("trial_xyz"));
    }

    /// FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: an empty
    /// `authoritative_judge_artifact_sha256` fails closed BEFORE any
    /// subprocess is spawned -- the exact P7C-authorized judge scope is
    /// required, never optional.
    #[test]
    fn empty_authoritative_judge_artifact_sha256_fails_closed_before_any_spawn() {
        let outcome = dsr_pbo_sensitivity_scenario(
            "mqk_this_executable_must_never_be_invoked",
            Path::new("/nonexistent/research-py"),
            Path::new("/nonexistent/registry.sqlite3"),
            "some_trial",
            "some_strategy",
            "",
            &[8, 10],
            0.25,
            0.25,
        );
        assert!(outcome.applicable);
        assert!(!outcome.passed);
        assert!(
            outcome
                .reason
                .unwrap_or_default()
                .contains("authoritative_judge_artifact_sha256 must not be empty")
        );
    }
}
