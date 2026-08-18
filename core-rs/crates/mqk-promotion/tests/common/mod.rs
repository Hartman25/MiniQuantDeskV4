//! P7C-REPAIR-02/-03 (mission Section 5A / FINAL WAVE-2 BLOCKER REPAIR Patch
//! B): shared test-only helper replacing the removed
//! `VerifiedPromotionOosEvidence::valid_for_testing` production bypass.
//! Every caller here constructs a genuinely valid, self-consistent,
//! REGISTRY-anchored evidence bundle -- a real temporary SQLite database
//! with real `research_trials`/`research_attempts`/`research_judge_artifacts`
//! rows -- and passes it through the REAL `verify_promotion_oos_evidence`,
//! which reads that registry read-only to establish authority. There is no
//! shortcut construction path left anywhere, in production or in tests.

use std::path::{Path, PathBuf};

use mqk_promotion::{verify_promotion_oos_evidence, VerifiedPromotionOosEvidence};
use rusqlite::Connection;
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

/// A temporary, real SQLite Research registry -- the same minimal schema
/// (subset of columns) `mqk_promotion::research_registry` reads from the
/// real `mqk_research.exp_distributed.storage.ResearchResultStore` database.
/// Kept alive for as long as `dir` is held; the file is deleted when it
/// drops.
pub struct RegistryDb {
    #[allow(dead_code)] // kept only to hold the TempDir alive for its Drop
    dir: tempfile::TempDir,
    pub path: PathBuf,
}

pub fn new_registry_db() -> RegistryDb {
    let dir = tempfile::tempdir().expect("create temp dir for registry db");
    let path = dir.path().join("registry.sqlite3");
    let conn = Connection::open(&path).expect("open registry db");
    conn.execute_batch(
        "
        create table research_trials (
            trial_id text primary key,
            experiment_id text not null,
            hypothesis_id text not null
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
            hypothesis_id text
        );
        ",
    )
    .expect("create registry schema");
    RegistryDb { dir, path }
}

pub fn register_trial(db_path: &Path, trial_id: &str, experiment_id: &str, hypothesis_id: &str) {
    let conn = Connection::open(db_path).expect("open registry db");
    conn.execute(
        "insert into research_trials (trial_id, experiment_id, hypothesis_id) values (?1, ?2, ?3)",
        rusqlite::params![trial_id, experiment_id, hypothesis_id],
    )
    .expect("insert research_trials row");
}

pub fn register_succeeded_attempt(db_path: &Path, attempt_id: &str, trial_id: &str, result_id: &str) {
    let conn = Connection::open(db_path).expect("open registry db");
    conn.execute(
        "insert into research_attempts (attempt_id, trial_id, status, result_id) \
         values (?1, ?2, 'succeeded', ?3)",
        rusqlite::params![attempt_id, trial_id, result_id],
    )
    .expect("insert research_attempts row");
}

pub fn register_judge_artifact(
    db_path: &Path,
    judge_artifact_sha256: &str,
    judge_id: &str,
    experiment_id: &str,
    hypothesis_id: Option<&str>,
) {
    let conn = Connection::open(db_path).expect("open registry db");
    conn.execute(
        "insert into research_judge_artifacts \
         (judge_artifact_sha256, judge_id, experiment_id, hypothesis_id) values (?1, ?2, ?3, ?4)",
        rusqlite::params![judge_artifact_sha256, judge_id, experiment_id, hypothesis_id],
    )
    .expect("insert research_judge_artifacts row");
}

/// A fully self-consistent, structurally valid, registry-anchored evidence
/// bundle for `trial_id` -- the ONLY shape `verify_promotion_oos_evidence`
/// accepts. Mirrors the real Python artifact schemas exactly at the field
/// paths the verifier reads, and registers matching trial/attempt/judge
/// rows in a real temporary registry database.
pub fn valid_oos_evidence_for_testing(trial_id: &str) -> VerifiedPromotionOosEvidence {
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = format!("econ_eval_{trial_id}");
    let experiment_id = format!("exp_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = canonical_json_sha256(&judge_json);

    let registry = new_registry_db();
    register_trial(&registry.path, trial_id, &experiment_id, &format!("hyp_{trial_id}"));
    register_succeeded_attempt(
        &registry.path,
        &format!("{trial_id}:att0001"),
        trial_id,
        &economic_eval_id,
    );
    // hypothesis_id=None -- this judge run is scoped to the whole
    // experiment, which covers the trial regardless of its own
    // hypothesis_id (see `research_registry::load_research_authority`).
    register_judge_artifact(&registry.path, &judge_sha, &format!("judge_{trial_id}"), &experiment_id, None);

    verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &judge_json)
        .expect("common::valid_oos_evidence_for_testing must build a genuinely valid bundle")
}
