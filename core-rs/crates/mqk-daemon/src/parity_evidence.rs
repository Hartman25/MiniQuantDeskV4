//! TV-03A: Shadow/live parity evidence manifest seam.
//!
//! Establishes the minimum real parity-evidence seam so the repo can
//! truthfully represent whether shadow/live parity evidence exists for the
//! currently configured artifact.
//!
//! Parity evidence is written by the Python TV-03 pipeline as
//! `parity_evidence.json` (schema `parity-v1`) into the same artifact
//! directory that contains `promoted_manifest.json`.
//!
//! # Outcome vocabulary
//!
//! - **`NotConfigured`** — no artifact path configured; parity evidence gate is
//!   not applicable.  This is honest absence: no artifact = no evidence seam.
//! - **`Absent`** — artifact path is configured but `parity_evidence.json` does
//!   not exist in the artifact directory.  Absent evidence ≠ parity proven.
//!   Always fail-closed for any gate that requires positive parity evidence.
//! - **`Invalid`** — `parity_evidence.json` exists but is structurally invalid:
//!   unreadable, not valid JSON, wrong `schema_version`, or missing required
//!   fields.  Always fail-closed.
//! - **`Present`** — `parity_evidence.json` is valid, `schema_version = "parity-v1"`.
//!   `live_trust_complete` is surfaced honestly (always `false` in current
//!   builds; the runtime does not fabricate positive trust claims).
//! - **`Unavailable`** — the evaluator itself could not be run.  Reserved for
//!   the panic-safe wrapper.  Always fail-closed.
//!
//! # Design rules
//!
//! - Pure evaluator [`evaluate_parity_evidence`]: no env reads, no network,
//!   no DB.  Deterministic given a fixed filesystem.
//! - Production entry point [`evaluate_parity_evidence_from_env`]: reads
//!   [`ENV_ARTIFACT_PATH`][crate::artifact_intake::ENV_ARTIFACT_PATH] and
//!   delegates.
//! - Panic-safe wrapper [`evaluate_parity_evidence_guarded`] surfaces any
//!   evaluator panic as `Unavailable`.
//!
//! # Forward chain
//!
//! TV-03C will use this seam at the start boundary to block start when parity
//! evidence is absent or invalid.

use std::path::Path;

/// Schema version string written by the Python TV-03 pipeline.
/// Must match `PARITY_EVIDENCE_CONTRACT_VERSION` in `contracts.py`.
const PARITY_EVIDENCE_SCHEMA_VERSION: &str = "parity-v1";

/// Filename written by the Python TV-03 pipeline inside the artifact directory.
const PARITY_EVIDENCE_FILENAME: &str = "parity_evidence.json";

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// Result of evaluating parity evidence for the configured artifact.
///
/// Only [`ParityEvidenceOutcome::NotConfigured`] and
/// [`ParityEvidenceOutcome::Present`] are non-blocking in normal operation.
/// All other variants represent evidence problems (absent, invalid,
/// unavailable) and are fail-closed when a gate requires positive evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParityEvidenceOutcome {
    /// No artifact path was configured (env var absent or empty).
    ///
    /// Parity evidence gate is not applicable.  Operators have not yet
    /// configured an artifact; the absence of evidence is honest.
    NotConfigured,

    /// Artifact path is configured but `parity_evidence.json` was not found
    /// in the artifact directory.
    ///
    /// Absent evidence ≠ parity proven.  The Python TV-03 pipeline has not
    /// produced parity evidence for this artifact.  Always fail-closed.
    Absent,

    /// `parity_evidence.json` was found but is structurally invalid:
    /// unreadable, not valid JSON, wrong `schema_version`, or missing required
    /// fields.
    ///
    /// Always fail-closed.
    Invalid {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// `parity_evidence.json` is valid and readable.  All required fields are
    /// present.
    ///
    /// `live_trust_complete` is surfaced honestly — the current Python TV-03
    /// pipeline always writes `false`; the daemon never fabricates positive
    /// trust claims.
    Present {
        /// Canonical artifact ID (TV-01).  Ties this evidence to a specific
        /// artifact.
        artifact_id: String,
        /// Whether the parity chain is complete enough for live capital.
        ///
        /// Always `false` in current builds.  Surfaced explicitly so operators
        /// can observe the current trust state rather than inferring it.
        live_trust_complete: bool,
        /// Whether shadow evaluation evidence was actually produced (as
        /// opposed to the evidence manifest noting that no shadow run has been
        /// run yet).
        evidence_available: bool,
        /// Human-readable description of what shadow evidence exists or is
        /// missing.
        evidence_note: String,
        /// ISO-8601 UTC string recording when this parity evidence was produced.
        produced_at_utc: String,
    },

    /// The parity evidence evaluator itself could not be run.
    ///
    /// Reserved for the panic-safe wrapper.  Always fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl ParityEvidenceOutcome {
    /// Truth-state label for the operator-visible control-plane surface.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Absent => "absent",
            Self::Invalid { .. } => "invalid",
            Self::Present { .. } => "present",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome represents positively-present parity evidence.
    ///
    /// `true` only for `Present` — all other variants mean evidence is absent,
    /// invalid, or unevaluable.
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present { .. })
    }

    /// Whether this outcome is start-safe (i.e. does not block a gate that
    /// requires positive parity evidence).
    ///
    /// `NotConfigured` is start-safe because the gate is not applicable when
    /// no artifact is configured.  `Present` with a live and valid evidence
    /// file is start-safe.  All other variants block.
    pub fn is_start_safe(&self) -> bool {
        matches!(self, Self::NotConfigured | Self::Present { .. })
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator
// ---------------------------------------------------------------------------

/// Evaluate parity evidence for the artifact at `artifact_manifest_path`.
///
/// Reads `parity_evidence.json` from the **parent directory** of
/// `artifact_manifest_path`.  Pure: no env reads, no network, no DB.
///
/// Pass `None` to represent an unconfigured artifact path.
///
/// # Validation contract
///
/// 1. `artifact_manifest_path` must be `Some` and non-empty — otherwise
///    `NotConfigured`.
/// 2. Parent directory must be derivable — otherwise `Invalid`.
/// 3. `parity_evidence.json` must exist in the parent directory — otherwise
///    `Absent` (absent evidence is not a positive parity claim).
/// 4. File must be valid JSON — otherwise `Invalid`.
/// 5. `schema_version` must equal `"parity-v1"` — otherwise `Invalid`.
/// 6. `artifact_id` must be present and non-empty — otherwise `Invalid`.
/// 7. `live_trust_complete` must be a boolean — otherwise `Invalid`.
/// 8. `shadow_evidence.evidence_available` must be a boolean — otherwise
///    `Invalid`.
/// 9. `shadow_evidence.evidence_note` must be a non-empty string — otherwise
///    `Invalid`.
/// 10. `produced_at_utc` must be present — otherwise `Invalid`.
/// 11. All fields present and valid → `Present`.
pub fn evaluate_parity_evidence(artifact_manifest_path: Option<&Path>) -> ParityEvidenceOutcome {
    let path = match artifact_manifest_path {
        None => return ParityEvidenceOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return ParityEvidenceOutcome::NotConfigured,
        Some(p) => p,
    };

    let evidence_path = match path.parent() {
        Some(parent) => parent.join(PARITY_EVIDENCE_FILENAME),
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: format!(
                    "cannot derive parent directory of '{}'; \
                     artifact_manifest_path must include a parent directory",
                    path.display()
                ),
            }
        }
    };

    let contents = match std::fs::read_to_string(&evidence_path) {
        Ok(s) => s,
        Err(_) => return ParityEvidenceOutcome::Absent,
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return ParityEvidenceOutcome::Invalid {
                reason: format!("invalid JSON in '{}': {e}", evidence_path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == PARITY_EVIDENCE_SCHEMA_VERSION => {}
        Some(other) => {
            return ParityEvidenceOutcome::Invalid {
                reason: format!(
                    "unsupported parity_evidence schema_version '{}'; expected '{}'",
                    other, PARITY_EVIDENCE_SCHEMA_VERSION
                ),
            }
        }
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing 'schema_version' field in parity_evidence.json".to_string(),
            }
        }
    }

    let artifact_id = match j
        .get("artifact_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        Some(id) => id.to_string(),
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing or empty 'artifact_id' field in parity_evidence.json".to_string(),
            }
        }
    };

    let live_trust_complete = match j.get("live_trust_complete").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason:
                    "missing or non-boolean 'live_trust_complete' field in parity_evidence.json"
                        .to_string(),
            }
        }
    };

    let shadow_obj = match j.get("shadow_evidence") {
        Some(v) => v,
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing 'shadow_evidence' object in parity_evidence.json".to_string(),
            }
        }
    };

    let evidence_available = match shadow_obj
        .get("evidence_available")
        .and_then(|v| v.as_bool())
    {
        Some(b) => b,
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing or non-boolean 'shadow_evidence.evidence_available' in \
                         parity_evidence.json"
                    .to_string(),
            }
        }
    };

    let evidence_note = match shadow_obj
        .get("evidence_note")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        Some(s) => s.to_string(),
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing or empty 'shadow_evidence.evidence_note' in \
                     parity_evidence.json"
                    .to_string(),
            }
        }
    };

    let produced_at_utc = match j
        .get("produced_at_utc")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        Some(s) => s.to_string(),
        None => {
            return ParityEvidenceOutcome::Invalid {
                reason: "missing or empty 'produced_at_utc' field in parity_evidence.json"
                    .to_string(),
            }
        }
    };

    ParityEvidenceOutcome::Present {
        artifact_id,
        live_trust_complete,
        evidence_available,
        evidence_note,
        produced_at_utc,
    }
}

// ---------------------------------------------------------------------------
// Production entry point
// ---------------------------------------------------------------------------

/// Read [`ENV_ARTIFACT_PATH`][crate::artifact_intake::ENV_ARTIFACT_PATH] from
/// the environment and evaluate parity evidence for the configured artifact.
///
/// Returns `NotConfigured` when the env var is absent or empty.
/// Delegates all validation to [`evaluate_parity_evidence`].
pub fn evaluate_parity_evidence_from_env() -> ParityEvidenceOutcome {
    let raw = std::env::var(crate::artifact_intake::ENV_ARTIFACT_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_parity_evidence(path.as_deref())
}

// ---------------------------------------------------------------------------
// Panic-safe wrapper
// ---------------------------------------------------------------------------

/// Panic-safe wrapper: run [`evaluate_parity_evidence_from_env`] and surface
/// any unexpected panic as [`ParityEvidenceOutcome::Unavailable`].
///
/// Route handlers should call this entry point so that an evaluator panic does
/// not crash the request handler thread.
pub fn evaluate_parity_evidence_guarded() -> ParityEvidenceOutcome {
    match std::panic::catch_unwind(evaluate_parity_evidence_from_env) {
        Ok(outcome) => outcome,
        Err(_) => ParityEvidenceOutcome::Unavailable {
            reason: "parity evidence evaluator panicked unexpectedly".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn next_id() -> u32 {
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn make_test_dir(tag: &str) -> std::path::PathBuf {
        let id = next_id();
        let dir = std::env::temp_dir().join(format!("mqk_tv03_{tag}_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_evidence(dir: &std::path::Path, content: &str) {
        let mut f = std::fs::File::create(dir.join(PARITY_EVIDENCE_FILENAME)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    fn manifest_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("promoted_manifest.json")
    }

    fn valid_evidence_json(artifact_id: &str) -> String {
        serde_json::json!({
            "schema_version": "parity-v1",
            "artifact_id": artifact_id,
            "gate_passed": true,
            "gate_schema_version": "gate-v1",
            "shadow_evidence": {
                "shadow_label_run_id": null,
                "labeled_rows": null,
                "precision": null,
                "recall": null,
                "f1": null,
                "evidence_available": false,
                "evidence_note": "No shadow evaluation run performed for this artifact"
            },
            "comparison_basis": "paper+alpaca supervised path",
            "live_trust_complete": false,
            "live_trust_gaps": ["TV-02 gate evaluates historical metrics only"],
            "produced_at_utc": "2026-03-01T00:00:00Z"
        })
        .to_string()
    }

    /// A01: not_configured when path is None.
    #[test]
    fn a01_not_configured_when_none() {
        let outcome = evaluate_parity_evidence(None);
        assert_eq!(outcome, ParityEvidenceOutcome::NotConfigured);
        assert_eq!(outcome.truth_state(), "not_configured");
        assert!(outcome.is_start_safe());
    }

    /// A02: absent when parity_evidence.json does not exist in artifact dir.
    #[test]
    fn a02_absent_when_file_missing() {
        let dir = make_test_dir("a02");
        let mp = manifest_path(&dir);
        // promoted_manifest.json exists but parity_evidence.json does not
        std::fs::File::create(&mp).unwrap();

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert_eq!(outcome, ParityEvidenceOutcome::Absent);
        assert_eq!(outcome.truth_state(), "absent");
        assert!(!outcome.is_start_safe());
        assert!(!outcome.is_present());
    }

    /// A03: invalid when parity_evidence.json is not valid JSON.
    #[test]
    fn a03_invalid_bad_json() {
        let dir = make_test_dir("a03");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        write_evidence(&dir, "not json {{{{");

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert!(matches!(outcome, ParityEvidenceOutcome::Invalid { .. }));
        assert_eq!(outcome.truth_state(), "invalid");
        assert!(!outcome.is_start_safe());
    }

    /// A04: invalid when schema_version is wrong.
    #[test]
    fn a04_invalid_wrong_schema_version() {
        let dir = make_test_dir("a04");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        write_evidence(
            &dir,
            r#"{"schema_version":"parity-v0","artifact_id":"test"}"#,
        );

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert!(
            matches!(outcome, ParityEvidenceOutcome::Invalid { ref reason } if reason.contains("parity-v0"))
        );
        assert_eq!(outcome.truth_state(), "invalid");
    }

    /// A05: present with live_trust_complete=false — honest, not fabricated.
    #[test]
    fn a05_present_live_trust_complete_is_false() {
        let dir = make_test_dir("a05");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        write_evidence(&dir, &valid_evidence_json("art-abc123"));

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert!(outcome.is_present(), "expected Present, got {:?}", outcome);
        assert_eq!(outcome.truth_state(), "present");
        assert!(outcome.is_start_safe());

        if let ParityEvidenceOutcome::Present {
            artifact_id,
            live_trust_complete,
            evidence_available,
            ..
        } = &outcome
        {
            assert_eq!(artifact_id, "art-abc123");
            assert!(!live_trust_complete, "live_trust_complete must be false");
            assert!(!evidence_available, "evidence_available=false in fixture");
        }
    }

    /// A06: evidence_available=false is distinct from Absent — evidence manifest
    /// exists and is readable, but the shadow run was not performed.
    #[test]
    fn a06_present_evidence_available_false_is_not_absent() {
        let dir = make_test_dir("a06");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        write_evidence(&dir, &valid_evidence_json("art-def456"));

        let outcome = evaluate_parity_evidence(Some(&mp));
        // Must be Present (file readable), not Absent.
        assert!(
            matches!(
                outcome,
                ParityEvidenceOutcome::Present {
                    evidence_available: false,
                    ..
                }
            ),
            "expected Present{{evidence_available:false}}, got {:?}",
            outcome
        );
    }

    /// A07: invalid when live_trust_complete field is missing.
    #[test]
    fn a07_invalid_missing_live_trust_complete() {
        let dir = make_test_dir("a07");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        let j = serde_json::json!({
            "schema_version": "parity-v1",
            "artifact_id": "art-x",
            "gate_passed": true,
            "gate_schema_version": "gate-v1",
            "shadow_evidence": {
                "evidence_available": false,
                "evidence_note": "no run"
            },
            "comparison_basis": "test",
            "produced_at_utc": "2026-01-01T00:00:00Z"
            // live_trust_complete intentionally absent
        });
        write_evidence(&dir, &j.to_string());

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert!(
            matches!(outcome, ParityEvidenceOutcome::Invalid { ref reason } if reason.contains("live_trust_complete")),
            "got: {:?}",
            outcome
        );
    }

    /// A08: invalid when shadow_evidence.evidence_note is missing.
    #[test]
    fn a08_invalid_missing_evidence_note() {
        let dir = make_test_dir("a08");
        let mp = manifest_path(&dir);
        std::fs::File::create(&mp).unwrap();
        let j = serde_json::json!({
            "schema_version": "parity-v1",
            "artifact_id": "art-y",
            "gate_passed": true,
            "gate_schema_version": "gate-v1",
            "shadow_evidence": {
                "evidence_available": false
                // evidence_note intentionally absent
            },
            "comparison_basis": "test",
            "live_trust_complete": false,
            "produced_at_utc": "2026-01-01T00:00:00Z"
        });
        write_evidence(&dir, &j.to_string());

        let outcome = evaluate_parity_evidence(Some(&mp));
        assert!(
            matches!(outcome, ParityEvidenceOutcome::Invalid { ref reason } if reason.contains("evidence_note")),
            "got: {:?}",
            outcome
        );
    }
}
