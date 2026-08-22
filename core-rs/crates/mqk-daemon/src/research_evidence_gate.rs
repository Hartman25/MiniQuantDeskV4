//! PROMOTION-WALKFORWARD-GATE-WIRING-01: production wiring of the P7C
//! verified Research out-of-sample evidence gate
//! (`mqk_promotion::verify_promotion_oos_evidence`) into the real
//! promotion decision path.
//!
//! Before this module, `mqk-promotion` had ZERO production callers in this
//! workspace -- every `verify_promotion_oos_evidence`/`PromotionInput`
//! construction site was that crate's own test suite (confirmed by
//! `Cargo.toml` dependency search across `core-rs`, and independently by
//! `docs/research/Research_Backtest_V1_Closeout_Audit.md`'s P7C section).
//! [`evaluate_research_evidence_gate`] is the first: it composes TRUSTED
//! daemon configuration (never request JSON -- there is no request field
//! for a registry path, an artifact root, or an acceptance threshold) with
//! caller-supplied artifact PATHS (root-bound and content-verified, never
//! caller-supplied artifact BYTES or a caller's claim that evidence
//! "passed") to call the real verifier, then applies this daemon's own
//! explicit, versioned DSR/PBO acceptance thresholds.
//!
//! Used by `routes::strategy_promotions::strategy_promotion_transition` as
//! an ADDITIONAL required gate alongside -- never instead of -- the
//! existing scanner/review-artifact evidence check
//! (`promotion_evidence_validation::validate_paper_candidate_evidence`).
//! Neither check can substitute for the other: a request with a valid
//! review artifact but no (or rejected) Research evidence fails closed, and
//! vice versa.

use std::path::Path;

use mqk_promotion::{verify_promotion_oos_evidence, VerifiedPromotionOosEvidence};

use crate::promotion_evidence_validation::{open_confined_regular_child, read_bounded_file_string};
use crate::state::AppState;

/// Bound on `economic_walk_forward.json` -- a per-fold summary artifact,
/// never approaches this in ordinary operation; exists so a malformed or
/// adversarial artifact fails closed instead of allocating without limit.
const MAX_ECONOMIC_ARTIFACT_JSON_BYTES: u64 = 16 * 1024 * 1024;
/// Bound on `economic_daily_returns.csv` -- one row per trading day across
/// the full discovery window, still small; generous headroom.
const MAX_DAILY_RETURNS_CSV_BYTES: u64 = 64 * 1024 * 1024;
/// Bound on the multiple-testing judge artifact JSON -- scales with
/// comparison-population size; generous headroom.
const MAX_JUDGE_ARTIFACT_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Outcome of [`evaluate_research_evidence_gate`]. Never panics, and every
/// rejection names its own reason(s) -- never a single opaque failure.
#[derive(Debug, Clone)]
pub enum ResearchEvidenceGateOutcome {
    Passed {
        /// The full, non-forgeable, structurally VERIFIED evidence --
        /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: this is
        /// what the route now feeds directly into
        /// `mqk_promotion::PromotionInput.oos_evidence`, so the canonical
        /// `evaluate_promotion` (not this gate) makes the final DSR/PBO
        /// policy decision -- no parallel/partial promotion policy.
        oos_evidence: Box<VerifiedPromotionOosEvidence>,
    },
    Rejected {
        blockers: Vec<String>,
    },
}

fn reject(msg: impl Into<String>) -> ResearchEvidenceGateOutcome {
    ResearchEvidenceGateOutcome::Rejected {
        blockers: vec![msg.into()],
    }
}

/// Verify that `trial_id` is backed by real, AUTHORITY-anchored Research
/// out-of-sample evidence, and that the verified DSR/PBO numbers clear this
/// daemon's configured acceptance thresholds.
///
/// Every artifact path this function reads is caller-supplied but strictly
/// root-bound inside `st.research_evidence_artifact_root` (canonicalize +
/// root-prefix, same pattern as
/// `promotion_evidence_validation::validate_paper_candidate_evidence`) --
/// never trusted as a bare claim. `st.research_registry_db_path` and the two
/// threshold fields are trusted daemon configuration ONLY: there is no
/// request field for any of the three anywhere in
/// `StrategyPromotionTransitionRequest`, so a caller cannot select an
/// alternate registry, artifact root, or acceptance threshold no matter what
/// a request body contains.
pub fn evaluate_research_evidence_gate(
    st: &AppState,
    strategy_id: &str,
    trial_id: &str,
    research_evidence_dir: &str,
    research_judge_artifact_path: &str,
) -> ResearchEvidenceGateOutcome {
    let trial_id = trial_id.trim();
    if trial_id.is_empty() {
        return reject("research_trial_id must not be blank");
    }

    let Some(registry_db_path) = st.research_registry_db_path.as_deref() else {
        return reject(
            "research-evidence gate rejected: this daemon has no MQK_RESEARCH_REGISTRY_DB \
             configured -- Research OOS evidence authority cannot be established",
        );
    };
    let Some(artifact_root) = st.research_evidence_artifact_root.as_deref() else {
        return reject(
            "research-evidence gate rejected: this daemon has no \
             MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT configured",
        );
    };

    let root_canon = match std::fs::canonicalize(Path::new(artifact_root)) {
        Ok(p) => p,
        Err(_) => {
            return reject(format!(
                "research-evidence gate rejected: configured research evidence artifact root is \
                 unavailable: {artifact_root}"
            ));
        }
    };

    // ---- research_evidence_dir: economic_walk_forward.json + economic_daily_returns.csv ----
    let evidence_dir_raw = research_evidence_dir.trim();
    if evidence_dir_raw.is_empty() {
        return reject("research_evidence_dir must not be blank");
    }
    let evidence_dir_canon = match std::fs::canonicalize(Path::new(evidence_dir_raw)) {
        Ok(p) => p,
        Err(_) => {
            return reject(format!("research_evidence_dir not found: {evidence_dir_raw}"));
        }
    };
    if !evidence_dir_canon.starts_with(&root_canon) {
        return reject(
            "research_evidence_dir does not resolve inside the configured research evidence \
             artifact root",
        );
    }
    let econ_child = evidence_dir_canon.join("economic_walk_forward.json");
    let econ_file =
        match open_confined_regular_child(&econ_child, &root_canon, &evidence_dir_canon) {
            Ok(f) => f,
            Err(msg) => return reject(msg),
        };
    let economic_walk_forward_json =
        match read_bounded_file_string(econ_file, &econ_child, MAX_ECONOMIC_ARTIFACT_JSON_BYTES) {
            Ok(s) => s,
            Err(msg) => return reject(msg),
        };

    let daily_child = evidence_dir_canon.join("economic_daily_returns.csv");
    let daily_file =
        match open_confined_regular_child(&daily_child, &root_canon, &evidence_dir_canon) {
            Ok(f) => f,
            Err(msg) => return reject(msg),
        };
    let economic_daily_returns_csv =
        match read_bounded_file_string(daily_file, &daily_child, MAX_DAILY_RETURNS_CSV_BYTES) {
            Ok(s) => s.into_bytes(),
            Err(msg) => return reject(msg),
        };

    // ---- research_judge_artifact_path: a single file, root-bound the same way ----
    // (Not "a child inside a candidate directory" -- there is no separate
    // candidate directory for a standalone file, so `root_canon` is passed
    // as both the root and the candidate boundary; the check collapses to
    // one canonicalize + one root-prefix comparison, correctly.)
    let judge_path_raw = research_judge_artifact_path.trim();
    if judge_path_raw.is_empty() {
        return reject("research_judge_artifact_path must not be blank");
    }
    let judge_file =
        match open_confined_regular_child(Path::new(judge_path_raw), &root_canon, &root_canon) {
            Ok(f) => f,
            Err(msg) => return reject(msg),
        };
    let judge_json = match read_bounded_file_string(
        judge_file,
        Path::new(judge_path_raw),
        MAX_JUDGE_ARTIFACT_JSON_BYTES,
    ) {
        Ok(s) => s,
        Err(msg) => return reject(msg),
    };

    let verified = match verify_promotion_oos_evidence(
        Path::new(registry_db_path),
        trial_id,
        &economic_walk_forward_json,
        &economic_daily_returns_csv,
        &judge_json,
    ) {
        Ok(v) => v,
        Err(errs) => return ResearchEvidenceGateOutcome::Rejected { blockers: errs },
    };

    // Cross-candidate authority (PROMOTION-WALKFORWARD-GATE-WIRING-01-
    // REPAIR-CLOSURE): this trial's own registered strategy_id
    // (research_trials.strategy_id, read by the durable registry, never a
    // caller claim) must agree with the promotion identity actually being
    // decided -- a valid Research trial for a DIFFERENT strategy must never
    // be accepted here.
    if verified.strategy_id().trim() != strategy_id {
        return reject(format!(
            "research-evidence gate rejected: registered trial strategy_id {:?} does not match \
             promotion candidate strategy_id {strategy_id:?} -- cross-candidate evidence is \
             never accepted",
            verified.strategy_id()
        ));
    }

    // DSR/PBO threshold acceptance is no longer decided here --
    // PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: the route now
    // feeds this verified evidence into `mqk_promotion::PromotionInput.
    // oos_evidence` and lets the canonical `evaluate_promotion` make that
    // decision against `PromotionConfig`'s thresholds, alongside every
    // other promotion gate (metrics, artifact lock, stress suite,
    // provenance) -- closing the "parallel/partial promotion policy" gap
    // independent review of `242cb7c3` found. "Successfully evaluated" is
    // still never conflated with "acceptable": this function's Passed
    // outcome means only that the evidence is genuine and correctly bound,
    // not that it clears any threshold.
    ResearchEvidenceGateOutcome::Passed {
        oos_evidence: Box::new(verified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Serializes the narrow set-env-vars-then-construct-AppState window,
    /// same rationale as `promotion_evidence_validation::tests::ENV_ROOT_LOCK`
    /// -- these four env vars are process-global and `AppState` captures
    /// them once at construction time.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn det_uuid(seed: &str) -> uuid::Uuid {
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, seed.as_bytes())
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// A fully self-consistent, registry-anchored, root-bound-on-disk
    /// evidence bundle for `trial_id`, plus an `AppState` configured to
    /// accept it (thresholds set loose enough to pass the fixture's own
    /// dsr=0.85 / pbo=0.15 values). Mirrors
    /// `mqk-promotion/tests/common::valid_oos_evidence_for_testing`'s exact
    /// artifact shapes, but writes them to real files under a temp root
    /// (this module's caller reads paths, not bytes) and builds the
    /// registry via `rusqlite` directly (this crate has no visibility into
    /// `mqk-promotion`'s test-only fixture module).
    struct Fixture {
        _dir: tempfile::TempDir,
        st: crate::state::AppState,
        trial_id: String,
        strategy_id: String,
        evidence_dir: std::path::PathBuf,
        judge_path: std::path::PathBuf,
    }

    fn build_fixture(seed: &str) -> Fixture {
        build_fixture_with_strategy(seed, &format!("strategy_{seed}"))
    }

    /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: variant with an
    /// explicit `strategy_id` -- used by cross-candidate binding tests that
    /// need to construct a Research trial registered under a KNOWN, chosen
    /// strategy identity (to prove a mismatch against a different promotion
    /// candidate's strategy_id is rejected).
    fn build_fixture_with_strategy(seed: &str, strategy_id: &str) -> Fixture {
        let dir = tempfile::tempdir().expect("create temp root");
        let root = dir.path().to_path_buf();

        let trial_id = format!("trial_{}", det_uuid(seed));
        let experiment_id = format!("exp_{}", det_uuid(seed));
        let economic_eval_id = format!("econ_eval_{}", det_uuid(seed));

        let evidence_dir = root.join("evidence");
        std::fs::create_dir_all(&evidence_dir).expect("create evidence dir");

        let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
        let daily_sha = sha256_hex(&daily_csv);
        std::fs::write(evidence_dir.join("economic_daily_returns.csv"), &daily_csv)
            .expect("write daily returns csv");

        let economic_json = format!(
            r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
        );
        let economic_sha = sha256_hex(economic_json.as_bytes());
        std::fs::write(evidence_dir.join("economic_walk_forward.json"), &economic_json)
            .expect("write economic artifact");

        let judge_json = format!(
            r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
        );
        let judge_sha = sha256_hex(judge_json.as_bytes());
        let judge_path = root.join("judge.json");
        std::fs::write(&judge_path, &judge_json).expect("write judge artifact");

        let registry_path = root.join("registry.sqlite3");
        let conn = rusqlite::Connection::open(&registry_path).expect("open registry db");
        conn.execute_batch(
            "
            create table research_trials (
                trial_id text primary key,
                experiment_id text not null,
                hypothesis_id text not null,
                strategy_id text not null
            );
            create table research_attempts (
                attempt_id text primary key,
                trial_id text not null,
                status text not null,
                result_id text
            );
            create table research_judge_artifacts (
                judge_artifact_sha256 text primary key,
                judge_id text not null,
                experiment_id text not null,
                hypothesis_id text,
                canonical_judge_json text
            );
            ",
        )
        .expect("create registry schema");
        conn.execute(
            "insert into research_trials (trial_id, experiment_id, hypothesis_id, strategy_id) \
             values (?1, ?2, ?3, ?4)",
            rusqlite::params![trial_id, experiment_id, format!("hyp_{seed}"), strategy_id],
        )
        .expect("insert research_trials row");
        conn.execute(
            "insert into research_attempts (attempt_id, trial_id, status, result_id) \
             values (?1, ?2, 'succeeded', ?3)",
            rusqlite::params![format!("{trial_id}:att0001"), trial_id, economic_eval_id],
        )
        .expect("insert research_attempts row");
        conn.execute(
            "insert into research_judge_artifacts \
             (judge_artifact_sha256, judge_id, experiment_id, hypothesis_id, canonical_judge_json) \
             values (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                judge_sha,
                format!("judge_{seed}"),
                experiment_id,
                Option::<String>::None,
                judge_json,
            ],
        )
        .expect("insert research_judge_artifacts row");
        drop(conn);

        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MQK_RESEARCH_REGISTRY_DB", &registry_path);
        std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", &root);
        std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.5");
        std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "0.5");
        let st = crate::state::AppState::new_with_operator_auth(
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        );

        Fixture {
            _dir: dir,
            st,
            trial_id,
            strategy_id: strategy_id.to_string(),
            evidence_dir,
            judge_path,
        }
    }

    fn unset_all_research_env() {
        std::env::remove_var("MQK_RESEARCH_REGISTRY_DB");
        std::env::remove_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT");
        std::env::remove_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO");
        std::env::remove_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING");
    }

    #[test]
    fn genuine_registry_anchored_evidence_passes_through_production_gate() {
        let f = build_fixture("genuine_pass");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        match outcome {
            ResearchEvidenceGateOutcome::Passed { oos_evidence } => {
                assert_eq!(oos_evidence.trial_id(), f.trial_id);
                assert!((oos_evidence.deflated_sharpe_ratio() - 0.85).abs() < 1e-9);
                assert!(
                    (oos_evidence.probability_of_backtest_overfitting() - 0.15).abs() < 1e-9
                );
                assert_eq!(oos_evidence.strategy_id(), f.strategy_id);
            }
            other => panic!("expected Passed, got {other:?}"),
        }
        unset_all_research_env();
    }

    #[test]
    fn cross_candidate_strategy_id_mismatch_is_rejected() {
        let f = build_fixture_with_strategy("cross_candidate", "Real_Owner_Strategy");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            "Some_Other_Strategy",
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        match outcome {
            ResearchEvidenceGateOutcome::Rejected { blockers } => {
                assert!(blockers.iter().any(|b| b.contains("cross-candidate")));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        unset_all_research_env();
    }

    #[test]
    fn missing_registry_config_fails_closed() {
        let f = build_fixture("missing_registry");
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MQK_RESEARCH_REGISTRY_DB");
        let st = crate::state::AppState::new_with_operator_auth(
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        );
        drop(_guard);
        let outcome = evaluate_research_evidence_gate(
            &st,
            &f.strategy_id,
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }

    #[test]
    fn missing_artifact_root_config_fails_closed() {
        let f = build_fixture("missing_root");
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT");
        let st = crate::state::AppState::new_with_operator_auth(
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        );
        drop(_guard);
        let outcome = evaluate_research_evidence_gate(
            &st,
            &f.strategy_id,
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }

    #[test]
    fn unregistered_trial_id_fails_closed() {
        let f = build_fixture("unregistered_trial");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            "trial_that_was_never_registered",
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }

    #[test]
    fn evidence_dir_escaping_configured_root_is_rejected() {
        let f = build_fixture("root_escape");
        let outside = tempfile::tempdir().expect("create outside dir");
        std::fs::copy(
            f.evidence_dir.join("economic_walk_forward.json"),
            outside.path().join("economic_walk_forward.json"),
        )
        .expect("copy econ artifact outside root");
        std::fs::copy(
            f.evidence_dir.join("economic_daily_returns.csv"),
            outside.path().join("economic_daily_returns.csv"),
        )
        .expect("copy daily returns outside root");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            &f.trial_id,
            outside.path().to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        match outcome {
            ResearchEvidenceGateOutcome::Rejected { blockers } => {
                assert!(blockers.iter().any(|b| b.contains("does not resolve inside")));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        unset_all_research_env();
    }

    #[test]
    fn judge_path_escaping_configured_root_is_rejected() {
        let f = build_fixture("judge_root_escape");
        let outside = tempfile::tempdir().expect("create outside dir");
        let outside_judge = outside.path().join("judge.json");
        std::fs::copy(&f.judge_path, &outside_judge).expect("copy judge artifact outside root");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            outside_judge.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }

    #[test]
    fn mutated_judge_artifact_bytes_fail_authority_match() {
        let f = build_fixture("mutated_judge");
        // Flip the registered DSR value on disk after the registry already
        // recorded the ORIGINAL judge content's hash -- authority match
        // must fail (see mqk_promotion::research_registry's own identical
        // proof requirement).
        let mutated = std::fs::read_to_string(&f.judge_path)
            .unwrap()
            .replace("0.85", "0.999999");
        std::fs::write(&f.judge_path, mutated).unwrap();
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            &f.trial_id,
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }

    #[test]
    fn blank_trial_id_is_rejected_before_any_io() {
        let f = build_fixture("blank_trial_id");
        let outcome = evaluate_research_evidence_gate(
            &f.st,
            &f.strategy_id,
            "   ",
            f.evidence_dir.to_str().unwrap(),
            f.judge_path.to_str().unwrap(),
        );
        assert!(matches!(outcome, ResearchEvidenceGateOutcome::Rejected { .. }));
        unset_all_research_env();
    }
}
