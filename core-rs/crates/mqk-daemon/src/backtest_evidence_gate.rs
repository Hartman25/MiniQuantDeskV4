//! PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: production wiring of
//! the canonical, candidate-bound backtest evidence resolver
//! (`mqk_promotion::resolve_backtest_evidence`,
//! `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`) into the real promotion decision
//! path.
//!
//! [`evaluate_backtest_evidence_gate`] composes TRUSTED daemon configuration
//! (`st.backtest_evidence_artifact_root` -- never request JSON) with a
//! caller-supplied candidate IDENTITY (`backtest_run_id`, never a path or
//! the evidence itself) to resolve the real `BacktestReport`/
//! `ArtifactLock`/`StressSuiteResult` bundle for one candidate, and
//! independently re-verifies that the resolved evidence's own
//! `strategy_name` agrees with the promotion identity's `strategy_id` --
//! closing the cross-candidate authority gap independent review of
//! `242cb7c3` found (a valid Backtest candidate must not be mixable with a
//! promotion identity it was never produced for).
//!
//! Used by `routes::strategy_promotions::strategy_promotion_transition`
//! alongside (never instead of) the scanner/review-artifact gate and the
//! Research OOS-evidence gate (`research_evidence_gate`). The resolved
//! bundle feeds `mqk_promotion::PromotionInput`, which the route then
//! decides via the canonical `mqk_promotion::evaluate_promotion` -- this
//! module never itself judges promotion eligibility.

use std::path::Path;

use mqk_promotion::{resolve_backtest_evidence, BacktestEvidenceBundle};
use uuid::Uuid;

use crate::state::AppState;

/// Outcome of [`evaluate_backtest_evidence_gate`]. Never panics, and every
/// rejection names its own reason(s) -- never a single opaque failure.
#[derive(Debug, Clone)]
pub enum BacktestEvidenceGateOutcome {
    Passed { bundle: Box<BacktestEvidenceBundle> },
    Rejected { blockers: Vec<String> },
}

fn reject(msg: impl Into<String>) -> BacktestEvidenceGateOutcome {
    BacktestEvidenceGateOutcome::Rejected {
        blockers: vec![msg.into()],
    }
}

/// Resolve and cross-candidate-bind the real backtest evidence for
/// `backtest_run_id`, and prove it was produced for the SAME `strategy_id`
/// as the promotion candidate being decided.
///
/// `st.backtest_evidence_artifact_root` is trusted daemon configuration
/// ONLY -- there is no request field for it anywhere in
/// `StrategyPromotionTransitionRequest`, so a caller cannot select an
/// alternate artifact root no matter what a request body contains.
pub fn evaluate_backtest_evidence_gate(
    st: &AppState,
    backtest_run_id: &str,
    strategy_id: &str,
) -> BacktestEvidenceGateOutcome {
    let backtest_run_id = backtest_run_id.trim();
    if backtest_run_id.is_empty() {
        return reject("backtest_run_id must not be blank");
    }
    let run_id = match Uuid::parse_str(backtest_run_id) {
        Ok(u) => u,
        Err(e) => {
            return reject(format!(
                "backtest-evidence gate rejected: backtest_run_id {backtest_run_id:?} is not a \
                 valid UUID: {e}"
            ))
        }
    };

    let Some(artifact_root) = st.backtest_evidence_artifact_root.as_deref() else {
        return reject(
            "backtest-evidence gate rejected: this daemon has no \
             MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT configured -- canonical Backtest evidence \
             authority cannot be established",
        );
    };

    let bundle = match resolve_backtest_evidence(Path::new(artifact_root), run_id) {
        Ok(b) => b,
        Err(e) => {
            return reject(format!("backtest-evidence gate rejected: {e}"));
        }
    };

    // Cross-candidate authority: the resolved evidence's own strategy_name
    // (BKT-PROV-01 identity, folded into run_id and cross-checked against
    // manifest.json by the resolver itself) must agree with the promotion
    // identity actually being decided -- a valid Backtest candidate for a
    // DIFFERENT strategy must never be accepted here.
    if bundle.report.strategy_name.trim() != strategy_id {
        return reject(format!(
            "backtest-evidence gate rejected: resolved evidence strategy_name {:?} does not \
             match promotion candidate strategy_id {strategy_id:?} -- cross-candidate evidence \
             is never accepted",
            bundle.report.strategy_name
        ));
    }

    BacktestEvidenceGateOutcome::Passed {
        bundle: Box::new(bundle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same rationale as `research_evidence_gate::tests::ENV_LOCK` /
    /// `promotion_evidence_validation::tests::ENV_ROOT_LOCK`:
    /// `MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT` is process-global and
    /// `AppState` captures it once at construction time.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn state_with_root(root: &std::path::Path) -> AppState {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT", root);
        let st = AppState::new_with_operator_auth(crate::state::OperatorAuthMode::ExplicitDevNoToken);
        std::env::remove_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT");
        st
    }

    fn state_without_root() -> AppState {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT");
        AppState::new_with_operator_auth(crate::state::OperatorAuthMode::ExplicitDevNoToken)
    }

    #[test]
    fn blank_backtest_run_id_is_rejected_before_any_io() {
        let root = std::env::temp_dir().join("mqk_beg01_blank_never_created");
        let st = state_with_root(&root);
        let outcome = evaluate_backtest_evidence_gate(&st, "   ", "any_strategy");
        assert!(matches!(outcome, BacktestEvidenceGateOutcome::Rejected { .. }));
    }

    #[test]
    fn malformed_uuid_is_rejected() {
        let root = std::env::temp_dir().join("mqk_beg01_malformed_never_created");
        let st = state_with_root(&root);
        let outcome = evaluate_backtest_evidence_gate(&st, "not-a-uuid", "any_strategy");
        assert!(matches!(outcome, BacktestEvidenceGateOutcome::Rejected { .. }));
    }

    #[test]
    fn missing_artifact_root_config_fails_closed() {
        let st = state_without_root();
        let outcome =
            evaluate_backtest_evidence_gate(&st, &Uuid::from_u128(1).to_string(), "any_strategy");
        assert!(matches!(outcome, BacktestEvidenceGateOutcome::Rejected { .. }));
    }

    #[test]
    fn nonexistent_candidate_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "mqk_beg01_nonexistent_root_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let st = state_with_root(&root);
        let outcome =
            evaluate_backtest_evidence_gate(&st, &Uuid::from_u128(2).to_string(), "any_strategy");
        assert!(matches!(outcome, BacktestEvidenceGateOutcome::Rejected { .. }));
        std::fs::remove_dir_all(&root).ok();
    }

    // ---- Real end-to-end fixture: genuine engine run + real artifacts ----

    use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
    use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

    const M: i64 = 1_000_000;

    struct HoldFlat {
        name: &'static str,
    }

    impl Strategy for HoldFlat {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new(self.name, 60)
        }

        fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
            StrategyOutput::new(vec![TargetPosition::new("ES", 0)])
        }
    }

    fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
        let p = price_usd * M;
        BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
    }

    /// Build a real, fully-persisted run_dir under a fresh root: manifest,
    /// audit trail (completion + stress-suite events), canonical report,
    /// and a real (trivially passing, flat/no-trade) stress suite. Returns
    /// (root, run_id).
    fn real_persisted_candidate(strategy_name: &'static str) -> (std::path::PathBuf, Uuid) {
        let bars = vec![
            flat_bar(1_700_000_060, 500),
            flat_bar(1_700_000_120, 501),
        ];
        let config = BacktestConfig::test_defaults();
        let initial_cash = config.initial_cash_micros;

        let mut engine = BacktestEngine::new(config.clone());
        engine
            .add_strategy(Box::new(HoldFlat { name: strategy_name }))
            .unwrap();
        let report = engine.run(&bars).expect("engine.run must succeed");

        let root = std::env::temp_dir().join(format!(
            "mqk_beg01_real_{}_{}",
            strategy_name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        let config_hash = report.config_id.to_string();
        let init_result =
            mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
                exports_root: &root,
                schema_version: 1,
                run_id: report.run_id,
                strategy_name: &report.strategy_name,
                engine_id: "mqk-backtest",
                mode: "backtest",
                timeframe: None,
                timeframe_secs: Some(60),
                git_hash: "beg01_test_git_hash",
                config_hash: &config_hash,
                host_fingerprint: "beg01_test_host",
                now_utc: chrono::Utc::now(),
            })
            .expect("init_run_artifacts must succeed");

        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
            .expect("write_backtest_report must succeed");

        let strategy_name_owned = strategy_name;
        let stress_output = mqk_backtest::run_backtest_stress_suite(&report, &config, &bars, || {
            Box::new(HoldFlat {
                name: strategy_name_owned,
            })
        });
        mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output)
            .expect("write_canonical_stress_suite must succeed");

        (root, report.run_id)
    }

    #[test]
    fn genuine_candidate_for_matching_strategy_passes_gate() {
        let (root, run_id) = real_persisted_candidate("Beg01GenuinePass");
        let st = state_with_root(&root);

        let outcome =
            evaluate_backtest_evidence_gate(&st, &run_id.to_string(), "Beg01GenuinePass");
        match outcome {
            BacktestEvidenceGateOutcome::Passed { bundle } => {
                assert_eq!(bundle.run_id, run_id);
                assert_eq!(bundle.report.strategy_name, "Beg01GenuinePass");
            }
            other => panic!("expected Passed, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_candidate_strategy_id_mismatch_is_rejected() {
        let (root, run_id) = real_persisted_candidate("Beg01RealOwner");
        let st = state_with_root(&root);

        // Real, structurally valid evidence -- but requested under a
        // DIFFERENT strategy_id than what actually produced it.
        let outcome =
            evaluate_backtest_evidence_gate(&st, &run_id.to_string(), "SomeOtherStrategy");
        match outcome {
            BacktestEvidenceGateOutcome::Rejected { blockers } => {
                assert!(blockers.iter().any(|b| b.contains("cross-candidate")));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
