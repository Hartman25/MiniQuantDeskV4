use anyhow::Result;
use std::fs;
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn scenario_run_artifacts_manifest_created() -> Result<()> {
    let tmp = tempdir()?;
    let exports_root = tmp.path();

    let run_id = Uuid::new_v4();

    let out = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root,
        schema_version: 1,
        run_id,
        engine_id: "MAIN",
        mode: "PAPER",
        git_hash: "deadbeef",
        config_hash: "cafebabe",
        host_fingerprint: "HOST|USER|os|arch",
    })?;

    // manifest exists
    assert!(out.manifest_path.exists(), "manifest.json should exist");

    // placeholders exist
    let run_dir = out.run_dir;
    assert!(run_dir.join("audit.jsonl").exists());
    assert!(run_dir.join("orders.csv").exists());
    assert!(run_dir.join("fills.csv").exists());
    assert!(run_dir.join("equity_curve.csv").exists());
    assert!(run_dir.join("metrics.json").exists());

    // manifest parses as JSON and contains run_id
    let raw = fs::read_to_string(out.manifest_path)?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    assert_eq!(v["run_id"].as_str().unwrap(), run_id.to_string());

    Ok(())
}
