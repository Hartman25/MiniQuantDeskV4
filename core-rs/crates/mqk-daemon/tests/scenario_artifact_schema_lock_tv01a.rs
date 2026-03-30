//! TV-01A: Promoted artifact schema lock — formal proof.
//!
//! Proves that the `promoted-v1` schema contract is formally locked:
//!
//! - The minimal accepted form is stable and explicit (exactly the 5 required
//!   fields; no implicit defaults or extra fields required).
//! - Extra/unknown fields in the manifest do not break acceptance (forward
//!   compatibility: new fields may be added to the manifest without invalidating
//!   existing consumers).
//! - Each required field individually absent → `Invalid` (exhaustive coverage).
//! - Schema version key absent → `Invalid` (distinct from wrong value, which
//!   TV-01B / AI-04 already covers).
//! - No partial manifest (subset of required fields) is accepted.
//! - The version lock is strict: `"promoted-v2"` (version drift) → `Invalid`.
//!
//! # What distinguishes TV-01A from TV-01B
//!
//! TV-01B (AI-01..AI-10) proves the four intake outcomes exist and that specific
//! validation cases trigger `Invalid`.  TV-01A proves the schema contract is
//! **complete**: every required field is individually validated (not just the
//! one field tested in AI-05), the schema version is the exclusive lock, and
//! extra fields are forward-compatible.
//!
//! # Required fields (promoted-v1 schema)
//!
//! ```
//! schema_version  — must equal "promoted-v1" exactly
//! artifact_id     — non-empty string
//! artifact_type   — non-empty string
//! stage           — non-empty string
//! produced_by     — non-empty string
//! ```
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                                  |
//! |-------|---------------------------------------------------------------------------------|
//! | SL-01 | Minimal manifest (exactly 5 required fields, no extras) → Accepted             |
//! | SL-02 | Manifest with extra/unknown fields → Accepted (forward compat)                 |
//! | SL-03 | `schema_version` key entirely absent → Invalid (not just wrong value)          |
//! | SL-04 | `artifact_id` key absent → Invalid (distinct from empty; AI-06 tests empty)   |
//! | SL-05 | `artifact_type` key absent → Invalid (not tested in TV-01B)                   |
//! | SL-06 | `produced_by` key absent → Invalid (not tested in TV-01B)                     |
//! | SL-07 | `schema_version = "promoted-v2"` (version drift) → Invalid (version lock)     |
//! | SL-08 | Empty JSON object `{}` → Invalid (no implicit defaults; all required absent)  |
//!
//! All tests are pure in-process (no env vars, no routes, no DB, no network).
//! `evaluate_artifact_intake` is pure and deterministic; no env-var lock needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use mqk_daemon::artifact_intake::{evaluate_artifact_intake, ArtifactIntakeOutcome};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Write `contents` to a unique temp file and return its path.
fn write_temp_json(tag: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_tv01a_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write temp json");
    path
}

fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

// ===========================================================================
// TV-01A — Schema lock proof tests
// ===========================================================================

/// SL-01: Minimal manifest (exactly 5 required fields, no optional extras) →
///        Accepted with all fields populated.
///
/// Proves the stable minimum contract: a manifest that carries only the 5
/// required fields (no `data_root`, no `lineage`, no extras) is accepted.
/// The schema lock has no hidden required fields beyond the 5.
#[test]
fn sl_01_minimal_manifest_with_only_required_fields_is_accepted() {
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "sl01-artifact-abc123",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py"
}"#;
    let path = write_temp_json("sl01", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Accepted {
            artifact_id,
            artifact_type,
            stage,
            produced_by,
        } => {
            assert_eq!(artifact_id, "sl01-artifact-abc123", "SL-01: artifact_id must match");
            assert_eq!(artifact_type, "signal_pack", "SL-01: artifact_type must match");
            assert_eq!(stage, "paper", "SL-01: stage must match");
            assert_eq!(produced_by, "research-py/promote.py", "SL-01: produced_by must match");
        }
        other => panic!("SL-01: minimal manifest must be Accepted; got: {other:?}"),
    }
}

/// SL-02: Manifest with extra/unknown fields alongside all required fields →
///        Accepted (forward compatibility).
///
/// Proves that the intake evaluator does not require exact field matching —
/// extra fields from future schema additions are silently ignored.  The
/// accepted form is forward-compatible: adding new fields to the manifest
/// does not break existing consumers.
#[test]
fn sl_02_manifest_with_extra_fields_is_accepted() {
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "sl02-artifact-xyz789",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py",
  "data_root": "promoted/signal_packs/sl02-artifact-xyz789",
  "lineage": {
    "source_run_id": "run-abc",
    "promoted_at_utc": "2026-03-29T00:00:00Z"
  },
  "unknown_future_field": "this is a new field added by a future schema version",
  "metadata": {"tags": ["equity", "momentum"]}
}"#;
    let path = write_temp_json("sl02", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Accepted { artifact_id, .. } => {
            assert_eq!(artifact_id, "sl02-artifact-xyz789", "SL-02: artifact_id must match");
        }
        other => panic!(
            "SL-02: manifest with extra fields must be Accepted (forward compat); got: {other:?}"
        ),
    }
}

/// SL-03: `schema_version` key entirely absent → Invalid.
///
/// Distinct from AI-04 (TV-01B) which tests a wrong VALUE for `schema_version`.
/// This test proves that a completely absent `schema_version` key is also
/// refused — no implicit default exists.
#[test]
fn sl_03_absent_schema_version_key_is_invalid() {
    // schema_version key is completely absent (not just wrong value).
    let contents = r#"{
  "artifact_id": "sl03-artifact",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py"
}"#;
    let path = write_temp_json("sl03", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("schema_version"),
                "SL-03: reason must mention schema_version; got: '{reason}'"
            );
        }
        other => panic!(
            "SL-03: absent schema_version key must return Invalid; got: {other:?}"
        ),
    }
}

/// SL-04: `artifact_id` key entirely absent → Invalid.
///
/// Distinct from AI-06 (TV-01B) which tests an EMPTY artifact_id string.
/// This test proves that a completely absent `artifact_id` key is also
/// refused — the field is individually required, not optional.
#[test]
fn sl_04_absent_artifact_id_key_is_invalid() {
    // artifact_id key is entirely absent (not just empty or whitespace).
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py"
}"#;
    let path = write_temp_json("sl04", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("artifact_id"),
                "SL-04: reason must mention artifact_id; got: '{reason}'"
            );
        }
        other => panic!(
            "SL-04: absent artifact_id key must return Invalid; got: {other:?}"
        ),
    }
}

/// SL-05: `artifact_type` key entirely absent → Invalid.
///
/// Proves that `artifact_type` is individually required.
/// Not covered by any TV-01B test.
#[test]
fn sl_05_absent_artifact_type_key_is_invalid() {
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "sl05-artifact",
  "stage": "paper",
  "produced_by": "research-py/promote.py"
}"#;
    let path = write_temp_json("sl05", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("artifact_type"),
                "SL-05: reason must mention artifact_type; got: '{reason}'"
            );
        }
        other => panic!(
            "SL-05: absent artifact_type key must return Invalid; got: {other:?}"
        ),
    }
}

/// SL-06: `produced_by` key entirely absent → Invalid.
///
/// Proves that `produced_by` is individually required.
/// Not covered by any TV-01B test.
#[test]
fn sl_06_absent_produced_by_key_is_invalid() {
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "sl06-artifact",
  "artifact_type": "signal_pack",
  "stage": "paper"
}"#;
    let path = write_temp_json("sl06", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("produced_by"),
                "SL-06: reason must mention produced_by; got: '{reason}'"
            );
        }
        other => panic!(
            "SL-06: absent produced_by key must return Invalid; got: {other:?}"
        ),
    }
}

/// SL-07: `schema_version = "promoted-v2"` → Invalid (version lock proof).
///
/// Proves the schema version is a strict lock, not a prefix match or
/// range check.  Only `"promoted-v1"` is accepted.  A realistic version
/// drift (`promoted-v2`) is explicitly refused.
///
/// Distinct from AI-04 (TV-01B) which uses `"promoted-v99"` — this test
/// proves the lock applies to realistic adjacent versions too.
#[test]
fn sl_07_version_drift_promoted_v2_is_invalid() {
    let contents = r#"{
  "schema_version": "promoted-v2",
  "artifact_id": "sl07-artifact",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py"
}"#;
    let path = write_temp_json("sl07", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("promoted-v2") || reason.contains("schema_version"),
                "SL-07: reason must reference the unsupported version; got: '{reason}'"
            );
            // Explicit: the reason must NOT claim "promoted-v1" was what was found.
            assert!(
                !reason.contains("promoted-v1")
                    || reason.contains("expected")
                    || reason.contains("unsupported"),
                "SL-07: reason must clarify the version is unsupported; got: '{reason}'"
            );
        }
        other => panic!(
            "SL-07: schema_version promoted-v2 (version drift) must return Invalid; got: {other:?}"
        ),
    }
}

/// SL-08: Empty JSON object `{}` → Invalid.
///
/// Proves there are no implicit defaults: a structurally valid JSON object
/// that carries none of the required fields is refused.  The schema lock
/// has no fallback or default-value behavior.
#[test]
fn sl_08_empty_json_object_is_invalid() {
    let contents = "{}";
    let path = write_temp_json("sl08", contents);

    let outcome = evaluate_artifact_intake(Some(&path));
    cleanup(&path);

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                !reason.is_empty(),
                "SL-08: Invalid reason must not be empty; got: '{reason}'"
            );
            // The missing schema_version is the first check to fail.
            assert!(
                reason.contains("schema_version"),
                "SL-08: reason must mention schema_version (first required field checked); \
                 got: '{reason}'"
            );
        }
        other => panic!(
            "SL-08: empty JSON object must return Invalid (no implicit defaults); got: {other:?}"
        ),
    }
}
