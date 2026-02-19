//! PATCH 15d — CLI run start + artifact creation integration test
//!
//! Validates: integration of mqk-config + mqk-artifacts (+ mqk-audit hash chain)
//!
//! GREEN when:
//! - After simulating run start, artifact directory + manifest.json exist.
//! - manifest.json contains matching config_hash from config loader.
//! - audit.jsonl is writable and hash chain verifies after appending events.
//! - config_hash in manifest matches what load_layered_yaml produces.

use anyhow::Result;
use mqk_artifacts::{init_run_artifacts, InitRunArtifactsArgs};
use mqk_audit::{verify_hash_chain, AuditWriter, VerifyResult};
use mqk_config::load_layered_yaml_from_strings;
use serde_json::json;
use std::fs;
use tempfile::tempdir;
use uuid::Uuid;

const TEST_BASE_YAML: &str = r#"
engine:
  engine_id: "MAIN"
  mode: "PAPER"
broker:
  name: "alpaca"
  keys_env:
    api_key: "ALPACA_API_KEY_PAPER"
    api_secret: "ALPACA_API_SECRET_PAPER"
risk:
  daily_loss_limit: 0.02
  max_drawdown: 0.18
"#;

#[test]
fn run_start_creates_artifacts_with_matching_config_hash() -> Result<()> {
    let tmp = tempdir()?;
    let exports_root = tmp.path().join("exports");

    // 1. Load config (simulating what CLI run start does)
    let loaded = load_layered_yaml_from_strings(&[TEST_BASE_YAML])?;
    let config_hash = &loaded.config_hash;

    let run_id = Uuid::new_v4();

    // 2. Init artifacts (simulating what CLI run start does)
    let out = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &exports_root,
        schema_version: 1,
        run_id,
        engine_id: "MAIN",
        mode: "PAPER",
        git_hash: "deadbeef01234567",
        config_hash,
        host_fingerprint: "test_host|test_user|linux|x86_64",
    })?;

    // 3. Verify manifest.json exists and parses
    assert!(out.manifest_path.exists(), "manifest.json must exist");

    let manifest_raw = fs::read_to_string(&out.manifest_path)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_raw)?;

    // 4. Verify config_hash in manifest matches what config loader produced
    let manifest_config_hash = manifest["config_hash"]
        .as_str()
        .expect("manifest should have config_hash");
    assert_eq!(
        manifest_config_hash, config_hash,
        "manifest config_hash must match config loader output"
    );

    // 5. Verify run_id matches
    let manifest_run_id = manifest["run_id"]
        .as_str()
        .expect("manifest should have run_id");
    assert_eq!(
        manifest_run_id,
        run_id.to_string(),
        "manifest run_id must match"
    );

    // 6. Verify all artifact placeholders exist
    let run_dir = &out.run_dir;
    assert!(
        run_dir.join("audit.jsonl").exists(),
        "audit.jsonl placeholder"
    );
    assert!(
        run_dir.join("orders.csv").exists(),
        "orders.csv placeholder"
    );
    assert!(run_dir.join("fills.csv").exists(), "fills.csv placeholder");
    assert!(
        run_dir.join("equity_curve.csv").exists(),
        "equity_curve.csv placeholder"
    );
    assert!(
        run_dir.join("metrics.json").exists(),
        "metrics.json placeholder"
    );

    // 7. Write audit events with hash chain and verify
    let audit_path = run_dir.join("audit.jsonl");
    {
        let mut writer = AuditWriter::new(&audit_path, true)?;
        writer.append(
            run_id,
            "RUNTIME",
            "RUN_STARTED",
            json!({
                "config_hash": config_hash,
                "engine_id": "MAIN",
                "mode": "PAPER"
            }),
        )?;
        writer.append(
            run_id,
            "RUNTIME",
            "RUN_STOPPED",
            json!({"reason": "test_complete"}),
        )?;
    }

    // 8. Verify audit hash chain is intact
    let verify_result = verify_hash_chain(&audit_path)?;
    assert_eq!(
        verify_result,
        VerifyResult::Valid { lines: 2 },
        "audit hash chain should be valid after writing events"
    );

    Ok(())
}

#[test]
fn config_hash_is_deterministic_across_artifact_init() -> Result<()> {
    // Load config twice, create artifacts twice with same inputs
    let tmp = tempdir()?;

    let loaded_a = load_layered_yaml_from_strings(&[TEST_BASE_YAML])?;
    let loaded_b = load_layered_yaml_from_strings(&[TEST_BASE_YAML])?;

    assert_eq!(
        loaded_a.config_hash, loaded_b.config_hash,
        "config hash must be deterministic"
    );

    let run_id = Uuid::new_v4();

    let out_a = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &tmp.path().join("a"),
        schema_version: 1,
        run_id,
        engine_id: "MAIN",
        mode: "PAPER",
        git_hash: "abc123",
        config_hash: &loaded_a.config_hash,
        host_fingerprint: "host",
    })?;

    let out_b = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &tmp.path().join("b"),
        schema_version: 1,
        run_id,
        engine_id: "MAIN",
        mode: "PAPER",
        git_hash: "abc123",
        config_hash: &loaded_b.config_hash,
        host_fingerprint: "host",
    })?;

    // Both manifests should have the same config_hash
    let manifest_a: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out_a.manifest_path)?)?;
    let manifest_b: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out_b.manifest_path)?)?;

    assert_eq!(
        manifest_a["config_hash"], manifest_b["config_hash"],
        "config_hash in both manifests must be identical"
    );

    Ok(())
}
