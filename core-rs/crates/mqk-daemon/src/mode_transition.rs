//! CC-03A: Canonical mode-transition state machine.
//!
//! Provides the single authoritative truth model for what mode transitions are
//! admissible, refused, or fail-closed in the MiniQuantDesk V4 control plane.
//!
//! # Design rules
//!
//! - Mode transitions require a controlled daemon restart with configuration
//!   reload.  Hot switching is architecturally unsupported.
//! - This function is the **only** place where (from, to) transition semantics
//!   are defined.  Routes and tests must not invent parallel transition logic.
//! - All 16 (4×4) [`crate::state::DeploymentMode`] pair combinations are
//!   explicitly covered.  No combination is silently accepted or treated as a
//!   no-op without an explicit verdict.
//!
//! # Verdict classes
//!
//! | Verdict                  | Meaning                                                       |
//! |--------------------------|---------------------------------------------------------------|
//! | `SameMode`               | `from == to`; no action needed                                |
//! | `AdmissibleWithRestart`  | Supported; controlled restart + listed preconditions required |
//! | `Refused`                | Architecture explicitly blocks this transition class          |
//! | `FailClosed`             | Intended but blocked by incomplete proof requirements         |
//!
//! # Architectural constraints encoded here
//!
//! - `Backtest` is a research mode; it is not a production daemon runtime mode.
//!   Transitions between `Backtest` and any production mode are `Refused`.
//! - `LiveCapital` execution requires a complete parity proof chain that is not
//!   yet architecturally closed (`TV-03: live_trust_complete = false`).  Upward
//!   transitions to `LiveCapital` are `FailClosed` until the proof is complete.
//! - Downward transitions (`LiveCapital`/`LiveShadow` → `Paper`/`LiveShadow`)
//!   are `AdmissibleWithRestart` but require explicit position-closure
//!   preconditions.

use crate::state::DeploymentMode;

// ---------------------------------------------------------------------------
// Static precondition slices
//
// Each AdmissibleWithRestart verdict carries a reference to one of these
// slices.  Static slices avoid heap allocation on the hot path and make the
// precondition text easy to audit in one place.
// ---------------------------------------------------------------------------

const PRECONDITIONS_PAPER_TO_LIVE_SHADOW: &[&str] = &[
    "Disarm the daemon (POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"}).",
    "Drain or cancel all pending outbox orders before shutdown.",
    "Provide a promoted artifact chain with a passing deployability gate (TV-01/TV-02).",
    "Provide parity evidence confirming shadow-tracking plausibility (TV-03).",
    "Stop the daemon (SIGTERM) and confirm a clean exit (exit code 0, no active run in DB).",
    "Set MQK_DAEMON_DEPLOYMENT_MODE=live-shadow and MQK_DAEMON_ADAPTER_ID=alpaca in the daemon configuration.",
    "Restart the daemon and confirm /api/v1/system/status reports alpaca_ws_continuity=live before starting a run.",
];

const PRECONDITIONS_LIVE_SHADOW_TO_PAPER: &[&str] = &[
    "Disarm the daemon (POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"}).",
    "Drain or cancel all pending outbox orders before shutdown.",
    "Confirm no open shadow positions remain or document explicit position acknowledgement.",
    "Stop the daemon (SIGTERM) and confirm a clean exit.",
    "Set MQK_DAEMON_DEPLOYMENT_MODE=paper and MQK_DAEMON_ADAPTER_ID=alpaca in the daemon configuration.",
    "Restart the daemon and verify /api/v1/system/status reports mode=paper.",
];

const PRECONDITIONS_LIVE_CAPITAL_TO_LIVE_SHADOW: &[&str] = &[
    "Disarm the daemon (POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"}).",
    "Close or explicitly transfer all open capital positions before shutdown.",
    "Drain or cancel all pending outbox orders before shutdown.",
    "Stop the daemon (SIGTERM) and confirm a clean exit.",
    "Set MQK_DAEMON_DEPLOYMENT_MODE=live-shadow and MQK_DAEMON_ADAPTER_ID=alpaca in the daemon configuration.",
    "Restart the daemon and verify /api/v1/system/status reports mode=live-shadow.",
];

const PRECONDITIONS_LIVE_CAPITAL_TO_PAPER: &[&str] = &[
    "Disarm the daemon (POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"}).",
    "Close or explicitly transfer all open capital positions before shutdown.",
    "Drain or cancel all pending outbox orders before shutdown.",
    "Stop the daemon (SIGTERM) and confirm a clean exit.",
    "Set MQK_DAEMON_DEPLOYMENT_MODE=paper and MQK_DAEMON_ADAPTER_ID=alpaca in the daemon configuration.",
    "Restart the daemon and verify /api/v1/system/status reports mode=paper.",
];

// ---------------------------------------------------------------------------
// ModeTransitionVerdict
// ---------------------------------------------------------------------------

/// Authoritative verdict for a (from, to) deployment-mode pair.
///
/// Produced exclusively by [`evaluate_mode_transition`].  All control-plane
/// code that needs to reason about mode transitions must use this type as the
/// truth source — no route-local or caller-local transition logic is permitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeTransitionVerdict {
    /// Source and target mode are the same; no transition action is required.
    SameMode,

    /// Transition is architecturally supported.
    ///
    /// Hot switching is not supported; a controlled daemon restart with
    /// configuration reload is required.  The `preconditions` slice lists the
    /// ordered steps the operator must satisfy before restarting.
    AdmissibleWithRestart {
        /// Ordered operator preconditions.  Never empty for this variant.
        preconditions: &'static [&'static str],
    },

    /// Transition is explicitly refused by the architecture.
    ///
    /// This is a permanent structural refusal — satisfying preconditions will
    /// not make this transition admissible.
    Refused { reason: &'static str },

    /// Transition is architecturally intended but is fail-closed until specific
    /// proof requirements are satisfied that are not currently met.
    ///
    /// The transition will become `AdmissibleWithRestart` once the blocking
    /// proof patches are closed.
    FailClosed { reason: &'static str },
}

impl ModeTransitionVerdict {
    /// Machine-readable verdict string for API responses and logging.
    ///
    /// One of: `"same_mode"`, `"admissible_with_restart"`, `"refused"`,
    /// `"fail_closed"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SameMode => "same_mode",
            Self::AdmissibleWithRestart { .. } => "admissible_with_restart",
            Self::Refused { .. } => "refused",
            Self::FailClosed { .. } => "fail_closed",
        }
    }

    /// Human-readable reason string.  Present for all variants.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::SameMode => "Source and target mode are the same; no transition needed.",
            Self::AdmissibleWithRestart { .. } => {
                "Transition is architecturally supported via controlled restart. \
                 See preconditions."
            }
            Self::Refused { reason } | Self::FailClosed { reason } => reason,
        }
    }

    /// Ordered precondition strings.  Non-empty only for `AdmissibleWithRestart`.
    pub fn preconditions(&self) -> &'static [&'static str] {
        match self {
            Self::AdmissibleWithRestart { preconditions } => preconditions,
            _ => &[],
        }
    }

    /// `true` only when the transition is `AdmissibleWithRestart`.
    pub fn is_admissible(&self) -> bool {
        matches!(self, Self::AdmissibleWithRestart { .. })
    }

    /// `true` for `Refused` or `FailClosed` (transition must not proceed).
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Refused { .. } | Self::FailClosed { .. })
    }
}

// ---------------------------------------------------------------------------
// Canonical evaluation function
// ---------------------------------------------------------------------------

/// Return the authoritative [`ModeTransitionVerdict`] for a (from, to) pair.
///
/// This is the **single canonical truth source** for mode-transition control-
/// plane semantics.  All 16 (4×4) [`DeploymentMode`] combinations are
/// explicitly covered by exhaustive match arms.  No combination is silently
/// treated as a no-op or optimistically accepted.
///
/// Callers must not re-implement or override these semantics locally.
pub fn evaluate_mode_transition(from: DeploymentMode, to: DeploymentMode) -> ModeTransitionVerdict {
    use DeploymentMode::*;
    use ModeTransitionVerdict::*;

    match (from, to) {
        // ── Same-mode: no action required ─────────────────────────────────
        (Paper, Paper)
        | (LiveShadow, LiveShadow)
        | (LiveCapital, LiveCapital)
        | (Backtest, Backtest) => SameMode,

        // ── Paper ↔ LiveShadow ────────────────────────────────────────────
        (Paper, LiveShadow) => AdmissibleWithRestart {
            preconditions: PRECONDITIONS_PAPER_TO_LIVE_SHADOW,
        },
        (LiveShadow, Paper) => AdmissibleWithRestart {
            preconditions: PRECONDITIONS_LIVE_SHADOW_TO_PAPER,
        },

        // ── LiveCapital → lower modes (admissible downgrades) ─────────────
        (LiveCapital, LiveShadow) => AdmissibleWithRestart {
            preconditions: PRECONDITIONS_LIVE_CAPITAL_TO_LIVE_SHADOW,
        },
        (LiveCapital, Paper) => AdmissibleWithRestart {
            preconditions: PRECONDITIONS_LIVE_CAPITAL_TO_PAPER,
        },

        // ── Upward transitions to LiveCapital: fail-closed ─────────────────
        //
        // LiveCapital execution is fail-closed until the parity proof chain is
        // architecturally complete.  TV-03 explicitly sets live_trust_complete=false.
        // This verdict must be updated when TV-01D closes the end-to-end proof.
        (Paper, LiveCapital) | (LiveShadow, LiveCapital) => FailClosed {
            reason: "LiveCapital execution is fail-closed: the end-to-end artifact → \
                     runtime consumption proof (TV-01D) is not yet complete and \
                     live_trust_complete=false in TV-03. This verdict will change to \
                     AdmissibleWithRestart once the proof chain is closed.",
        },

        // ── Backtest: structurally refused in all directions ───────────────
        //
        // Backtest is a research mode.  The daemon runtime does not support
        // Backtest as a production deployment target (deployment_mode_readiness
        // returns start_allowed=false for Backtest unconditionally).  Transitions
        // to/from Backtest are permanently refused; no precondition list applies.
        (Backtest, Paper)
        | (Backtest, LiveShadow)
        | (Backtest, LiveCapital)
        | (Paper, Backtest)
        | (LiveShadow, Backtest)
        | (LiveCapital, Backtest) => Refused {
            reason: "Backtest is a research mode; it is not a supported production daemon \
                     runtime target. Transitions between production modes and Backtest are \
                     not admissible in this architecture.",
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DeploymentMode;

    /// Verify the `as_str()` labels are stable and non-overlapping.
    #[test]
    fn verdict_as_str_labels_are_canonical() {
        assert_eq!(ModeTransitionVerdict::SameMode.as_str(), "same_mode");
        assert_eq!(
            ModeTransitionVerdict::AdmissibleWithRestart { preconditions: &[] }.as_str(),
            "admissible_with_restart"
        );
        assert_eq!(
            ModeTransitionVerdict::Refused { reason: "r" }.as_str(),
            "refused"
        );
        assert_eq!(
            ModeTransitionVerdict::FailClosed { reason: "r" }.as_str(),
            "fail_closed"
        );
    }

    /// All same-mode pairs → SameMode.
    #[test]
    fn same_mode_pairs_return_same_mode() {
        for mode in [
            DeploymentMode::Paper,
            DeploymentMode::LiveShadow,
            DeploymentMode::LiveCapital,
            DeploymentMode::Backtest,
        ] {
            let v = evaluate_mode_transition(mode, mode);
            assert_eq!(
                v,
                ModeTransitionVerdict::SameMode,
                "{mode:?} → {mode:?} must be SameMode"
            );
        }
    }

    /// Paper → LiveShadow is AdmissibleWithRestart with non-empty preconditions.
    #[test]
    fn paper_to_live_shadow_is_admissible_with_restart() {
        let v = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveShadow);
        assert!(
            v.is_admissible(),
            "Paper→LiveShadow must be admissible; got {:?}",
            v.as_str()
        );
        assert!(
            !v.preconditions().is_empty(),
            "Paper→LiveShadow must have non-empty preconditions"
        );
        assert!(
            v.preconditions()
                .iter()
                .any(|p| p.contains("artifact") || p.contains("parity")),
            "Paper→LiveShadow preconditions must mention artifact/parity requirements"
        );
    }

    /// LiveShadow → Paper is AdmissibleWithRestart (downgrade).
    #[test]
    fn live_shadow_to_paper_is_admissible_with_restart() {
        let v = evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::Paper);
        assert!(
            v.is_admissible(),
            "LiveShadow→Paper must be admissible; got {:?}",
            v.as_str()
        );
        assert!(!v.preconditions().is_empty());
    }

    /// LiveCapital → LiveShadow is AdmissibleWithRestart (downgrade).
    #[test]
    fn live_capital_to_live_shadow_is_admissible_with_restart() {
        let v = evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::LiveShadow);
        assert!(
            v.is_admissible(),
            "LiveCapital→LiveShadow must be admissible; got {:?}",
            v.as_str()
        );
        assert!(
            v.preconditions()
                .iter()
                .any(|p| p.contains("capital positions")),
            "LiveCapital downgrade must require closing capital positions"
        );
    }

    /// LiveCapital → Paper is AdmissibleWithRestart (downgrade).
    #[test]
    fn live_capital_to_paper_is_admissible_with_restart() {
        let v = evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::Paper);
        assert!(
            v.is_admissible(),
            "LiveCapital→Paper must be admissible; got {:?}",
            v.as_str()
        );
    }

    /// Paper → LiveCapital is FailClosed (proof gap).
    #[test]
    fn paper_to_live_capital_is_fail_closed() {
        let v = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
        assert_eq!(
            v.as_str(),
            "fail_closed",
            "Paper→LiveCapital must be fail_closed; got {:?}",
            v.as_str()
        );
        assert!(v.is_blocked());
        assert!(
            v.reason().contains("live_trust_complete=false") || v.reason().contains("fail-closed"),
            "FailClosed reason must reference live_trust_complete; got: {:?}",
            v.reason()
        );
    }

    /// LiveShadow → LiveCapital is FailClosed (same proof gap).
    #[test]
    fn live_shadow_to_live_capital_is_fail_closed() {
        let v = evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::LiveCapital);
        assert_eq!(v.as_str(), "fail_closed");
        assert!(v.is_blocked());
    }

    /// All transitions to/from Backtest are Refused.
    #[test]
    fn backtest_transitions_are_refused() {
        let production_modes = [
            DeploymentMode::Paper,
            DeploymentMode::LiveShadow,
            DeploymentMode::LiveCapital,
        ];
        for &mode in &production_modes {
            let v_to = evaluate_mode_transition(mode, DeploymentMode::Backtest);
            assert_eq!(
                v_to.as_str(),
                "refused",
                "{mode:?}→Backtest must be refused; got {:?}",
                v_to.as_str()
            );
            assert!(v_to.is_blocked());

            let v_from = evaluate_mode_transition(DeploymentMode::Backtest, mode);
            assert_eq!(
                v_from.as_str(),
                "refused",
                "Backtest→{mode:?} must be refused; got {:?}",
                v_from.as_str()
            );
            assert!(v_from.is_blocked());
        }
    }

    /// All 16 combinations return a non-default (explicit) verdict — completeness check.
    #[test]
    fn all_16_combinations_are_explicitly_covered() {
        let all = [
            DeploymentMode::Paper,
            DeploymentMode::LiveShadow,
            DeploymentMode::LiveCapital,
            DeploymentMode::Backtest,
        ];
        for &from in &all {
            for &to in &all {
                // Just calling this must not panic — Rust exhaustiveness ensures coverage.
                let v = evaluate_mode_transition(from, to);
                // The verdict string must be one of the four canonical values.
                assert!(
                    matches!(
                        v.as_str(),
                        "same_mode" | "admissible_with_restart" | "refused" | "fail_closed"
                    ),
                    "({from:?}→{to:?}) produced unexpected verdict string: {:?}",
                    v.as_str()
                );
            }
        }
    }

    /// Symmetry check: SameMode is always symmetric; blocked verdicts are asymmetric
    /// only where the architecture requires it (e.g. upward vs downward capital transitions).
    #[test]
    fn same_mode_is_symmetric() {
        let all = [
            DeploymentMode::Paper,
            DeploymentMode::LiveShadow,
            DeploymentMode::LiveCapital,
            DeploymentMode::Backtest,
        ];
        for &mode in &all {
            // (m, m) → SameMode; (m, m) evaluated in both directions is trivially symmetric.
            assert_eq!(
                evaluate_mode_transition(mode, mode),
                ModeTransitionVerdict::SameMode
            );
        }
    }

    /// Proves that upward → LiveCapital is FailClosed while downward LiveCapital → is
    /// AdmissibleWithRestart — the asymmetry is deliberate and must not regress.
    #[test]
    fn live_capital_asymmetry_is_canonical() {
        // Upward: fail-closed.
        assert_eq!(
            evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital).as_str(),
            "fail_closed"
        );
        assert_eq!(
            evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::LiveCapital)
                .as_str(),
            "fail_closed"
        );
        // Downward: admissible with restart.
        assert_eq!(
            evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::Paper).as_str(),
            "admissible_with_restart"
        );
        assert_eq!(
            evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::LiveShadow)
                .as_str(),
            "admissible_with_restart"
        );
    }
}
