//! P7C-REPAIR-02 (mission Section 5A): shared test-only helper replacing
//! the removed `VerifiedPromotionOosEvidence::valid_for_testing` production
//! bypass. Every caller here constructs a genuinely valid, self-consistent,
//! AUTHORITY-anchored evidence bundle and passes it through the REAL
//! `verify_promotion_oos_evidence` -- there is no shortcut construction
//! path left anywhere, in production or in tests.

use mqk_promotion::{verify_promotion_oos_evidence, ResearchAttemptAuthority, VerifiedPromotionOosEvidence};
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Matches `verify_promotion_oos_evidence`'s own canonical judge-artifact
/// hashing exactly (parse -> re-serialize via serde_json::Value, which
/// sorts object keys since this workspace never enables serde_json's
/// "preserve_order" feature -> hash) -- see that function's own doc
/// comment on why the authority hash must be canonical, not raw-bytes.
pub fn canonical_json_sha256(json_str: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(json_str).expect("valid JSON fixture");
    sha256_hex(&serde_json::to_vec(&value).expect("Value always re-serializes"))
}

/// A fully self-consistent, structurally valid, authority-anchored evidence
/// bundle for `trial_id` -- the ONLY shape `verify_promotion_oos_evidence`
/// accepts. Mirrors the real Python artifact schemas exactly at the field
/// paths the verifier reads.
pub fn valid_oos_evidence_for_testing(trial_id: &str) -> VerifiedPromotionOosEvidence {
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = format!("econ_eval_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"exp_{trial_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = canonical_json_sha256(&judge_json);

    let authority = ResearchAttemptAuthority {
        trial_id: trial_id.to_string(),
        economic_eval_id,
        judge_artifact_sha256: judge_sha,
    };

    verify_promotion_oos_evidence(&authority, trial_id, &economic_json, &daily_csv, &judge_json)
        .expect("common::valid_oos_evidence_for_testing must build a genuinely valid bundle")
}
