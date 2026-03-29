//! TV-01B: Runtime artifact intake contract.
//!
//! Establishes the minimum runtime artifact intake seam: a pure function that
//! reads a `promoted_manifest.json` (written by the Python TV-01 promotion
//! pipeline) and returns one of four honest outcomes:
//!
//! - **`NotConfigured`** — no artifact path is configured; operator hasn't
//!   provided an artifact.  This is the honest "no artifact" state; callers
//!   must not treat it as implicit permission to start without one.
//! - **`Invalid`** — path was configured but the file is unreadable, not valid
//!   JSON, or structurally invalid (wrong schema_version, missing required
//!   fields).  Fail-closed.
//! - **`Accepted`** — the file exists, is valid JSON, carries
//!   `schema_version = "promoted-v1"`, and has all required fields populated.
//!   This is *intake acceptance only* — it does not prove deployability,
//!   tradability, or profitability.
//! - **`Unavailable`** — the intake evaluator itself could not be run (e.g.,
//!   the evaluator panicked or an unrecoverable infrastructure failure
//!   prevented evaluation).  Always fail-closed: the daemon must not proceed
//!   as if artifact intake status is known.
//!
//! The public env-var constant [`ENV_ARTIFACT_PATH`] names the operator
//! configuration point.  All business logic lives in
//! [`evaluate_artifact_intake`], which is pure (no env reads, no IO) so it
//! can be tested deterministically.  [`evaluate_artifact_intake_from_env`] is
//! the production entry point: reads the env var and delegates.
//!
//! # Forward compatibility
//! - TV-01C will thread the `artifact_id` from an `Accepted` outcome into the
//!   run-start provenance record.
//! - TV-01D will prove the promoted artifact → runtime consumption chain end
//!   to end.

use std::path::Path;

// ---------------------------------------------------------------------------
// Deployability gate constants
// ---------------------------------------------------------------------------

/// The only accepted schema version for `deployability_gate.json`.
/// Must match TV-02 Python output (`gate-v1`).
const DEPLOYABILITY_GATE_SCHEMA_VERSION: &str = "gate-v1";

// ---------------------------------------------------------------------------
// ArtifactDeployabilityOutcome
// ---------------------------------------------------------------------------

/// Result of evaluating the TV-02 deployability gate for an accepted artifact.
///
/// Distinct from [`ArtifactIntakeOutcome`]: intake proves structural acceptance;
/// deployability proves the artifact meets minimum tradability/sample criteria
/// as evaluated by the Python TV-02 pipeline.
///
/// Only `Deployable` is start-safe at the runtime admission boundary.
/// All other variants are fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactDeployabilityOutcome {
    /// No artifact path was configured.  Gate is not applicable.
    ///
    /// Callers must treat this as "gate not evaluated", not as permission to
    /// start without a gate.
    NotConfigured,

    /// Artifact intake was `Accepted` but no `deployability_gate.json` was
    /// found in the artifact directory.
    ///
    /// Absent gate ≠ deployable.  Always fail-closed.
    GateAbsent,

    /// Gate file was found but is structurally invalid: unreadable, not valid
    /// JSON, wrong `schema_version`, missing required fields, or `artifact_id`
    /// mismatch with the intake artifact.
    ///
    /// Always fail-closed.
    GateInvalid {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// Gate file is valid but `passed = false`.
    ///
    /// The artifact did not meet the minimum tradability / sample adequacy
    /// criteria evaluated by the TV-02 Python pipeline.
    NotDeployable {
        /// The `overall_reason` string from the gate file.
        overall_reason: String,
    },

    /// Gate file is valid, `passed = true`, `artifact_id` verified against
    /// the intake artifact.
    ///
    /// This is the only start-safe outcome.
    Deployable {
        /// The artifact identity that cleared the gate.
        artifact_id: String,
    },

    /// The deployability evaluator itself could not be run.
    ///
    /// Always fail-closed: deployability status is unknown.
    Unavailable {
        /// Human-readable reason for the evaluation failure.
        reason: String,
    },
}

impl ArtifactDeployabilityOutcome {
    /// Truth-state label for the control-plane surface.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::GateAbsent => "gate_absent",
            Self::GateInvalid { .. } => "gate_invalid",
            Self::NotDeployable { .. } => "not_deployable",
            Self::Deployable { .. } => "deployable",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome allows runtime start.
    ///
    /// Returns `true` only for `Deployable`.  All other variants are
    /// fail-closed: they must block runtime start.
    pub fn is_deployable(&self) -> bool {
        matches!(self, Self::Deployable { .. })
    }
}

// ---------------------------------------------------------------------------
// Pure deployability evaluator
// ---------------------------------------------------------------------------

/// Evaluate the TV-02 deployability gate for an accepted artifact.
///
/// Reads `deployability_gate.json` from the **parent directory** of
/// `artifact_manifest_path`.  Pure: no env reads, no network, no DB.
///
/// # Parameters
/// - `artifact_manifest_path`: path to `promoted_manifest.json`.  `None` →
///   [`ArtifactDeployabilityOutcome::NotConfigured`].
/// - `accepted_artifact_id`: the `artifact_id` returned by the intake seam.
///   The gate file must carry the same value; a mismatch yields
///   [`ArtifactDeployabilityOutcome::GateInvalid`].
///
/// # Validation contract
/// 1. `artifact_manifest_path` must be `Some` and non-empty — otherwise `NotConfigured`.
/// 2. Parent directory must be derivable — otherwise `GateInvalid`.
/// 3. `deployability_gate.json` must exist in the parent directory — otherwise
///    `GateAbsent` (absent gate ≠ deployable; fail-closed).
/// 4. File must be valid JSON — otherwise `GateInvalid`.
/// 5. `schema_version` must equal `"gate-v1"` — otherwise `GateInvalid`.
/// 6. `artifact_id` must be present, non-empty, and match `accepted_artifact_id`
///    — otherwise `GateInvalid`.
/// 7. `passed` must be a boolean; `false` → `NotDeployable`.
/// 8. `passed = true` → `Deployable`.
pub fn evaluate_artifact_deployability(
    artifact_manifest_path: Option<&Path>,
    accepted_artifact_id: &str,
) -> ArtifactDeployabilityOutcome {
    let path = match artifact_manifest_path {
        None => return ArtifactDeployabilityOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return ArtifactDeployabilityOutcome::NotConfigured,
        Some(p) => p,
    };

    let gate_path = match path.parent() {
        Some(parent) => parent.join("deployability_gate.json"),
        None => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: format!(
                    "cannot derive parent directory of '{}'; \
                     artifact_manifest_path must include a parent directory",
                    path.display()
                ),
            }
        }
    };

    let contents = match std::fs::read_to_string(&gate_path) {
        Ok(s) => s,
        Err(_) => return ArtifactDeployabilityOutcome::GateAbsent,
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: format!("invalid JSON in '{}': {e}", gate_path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == DEPLOYABILITY_GATE_SCHEMA_VERSION => {}
        Some(other) => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: format!(
                    "unsupported gate schema_version '{}'; expected '{}'",
                    other, DEPLOYABILITY_GATE_SCHEMA_VERSION
                ),
            }
        }
        None => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: "gate file missing 'schema_version' field".to_string(),
            }
        }
    }

    let gate_artifact_id = match j.get("artifact_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.to_string(),
        _ => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: "gate file missing or empty 'artifact_id'".to_string(),
            }
        }
    };

    if gate_artifact_id != accepted_artifact_id {
        return ArtifactDeployabilityOutcome::GateInvalid {
            reason: format!(
                "gate artifact_id '{}' does not match accepted intake artifact_id '{}'",
                gate_artifact_id, accepted_artifact_id
            ),
        };
    }

    let passed = match j.get("passed").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return ArtifactDeployabilityOutcome::GateInvalid {
                reason: "gate file missing or non-boolean 'passed' field".to_string(),
            }
        }
    };

    if !passed {
        let overall_reason = j
            .get("overall_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("gate passed=false; no overall_reason provided")
            .to_string();
        return ArtifactDeployabilityOutcome::NotDeployable { overall_reason };
    }

    ArtifactDeployabilityOutcome::Deployable {
        artifact_id: gate_artifact_id,
    }
}

/// Env var the operator sets to the path of the `promoted_manifest.json` file.
///
/// Example: `MQK_ARTIFACT_PATH=/home/user/promoted/signal_packs/<id>/promoted_manifest.json`
pub const ENV_ARTIFACT_PATH: &str = "MQK_ARTIFACT_PATH";

/// The only accepted schema version string.  Must match TV-01 Python output.
const PROMOTED_ARTIFACT_SCHEMA_VERSION: &str = "promoted-v1";

// ---------------------------------------------------------------------------
// Outcome type
// ---------------------------------------------------------------------------

/// Result of evaluating a promoted artifact for runtime intake.
///
/// Only `Accepted` carries positive intake truth.  All other variants are
/// fail-closed: callers must not proceed as if an artifact is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactIntakeOutcome {
    /// No artifact path was configured (env var absent or empty).
    ///
    /// Honest absence: operator has not provided an artifact.  Must NOT be
    /// treated as "intake succeeded with no artifact".
    NotConfigured,

    /// Path was configured but the file is unreadable, not valid JSON, or
    /// structurally invalid (wrong schema_version, missing required fields).
    ///
    /// Always fail-closed: the daemon cannot verify artifact identity.
    Invalid {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// The promoted_manifest.json was read successfully and all required
    /// fields are present and non-empty.
    ///
    /// This is **intake acceptance only** — it does not imply deployability,
    /// tradability, or that the artifact has passed any economic gate.
    Accepted {
        /// Content-addressed artifact identity (sha256-derived).
        artifact_id: String,
        /// Artifact type string (e.g. `"signal_pack"`).
        artifact_type: String,
        /// Promotion stage the artifact was promoted to (e.g. `"paper"`).
        stage: String,
        /// Producing system identifier (e.g. `"research-py/promote.py"`).
        produced_by: String,
    },

    /// The intake evaluator itself could not be run.
    ///
    /// Surfaced when the evaluator panics or encounters an unrecoverable
    /// infrastructure failure that prevents evaluation entirely.  Distinct
    /// from `Invalid` (which requires evaluation to complete).
    ///
    /// Always fail-closed: intake status is unknown; callers must not proceed
    /// as if an artifact is available or absent.
    Unavailable {
        /// Human-readable reason for the evaluation failure.
        reason: String,
    },
}

impl ArtifactIntakeOutcome {
    /// Truth-state label for the control-plane surface.
    ///
    /// Maps to `ArtifactIntakeResponse.truth_state`.
    pub fn truth_state(&self) -> &'static str {
        match self {
            ArtifactIntakeOutcome::NotConfigured => "not_configured",
            ArtifactIntakeOutcome::Invalid { .. } => "invalid",
            ArtifactIntakeOutcome::Accepted { .. } => "accepted",
            ArtifactIntakeOutcome::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome represents a structurally accepted artifact.
    ///
    /// Returns `false` for `NotConfigured`, `Invalid`, and `Unavailable`.
    pub fn is_accepted(&self) -> bool {
        matches!(self, ArtifactIntakeOutcome::Accepted { .. })
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator
// ---------------------------------------------------------------------------

/// Evaluate a `promoted_manifest.json` at `path` and return the intake outcome.
///
/// This function is pure: no env-var reads, no network, no DB.  Pass
/// `None` to represent an unconfigured path.
///
/// # Validation contract
/// 1. `path` must be `Some` and non-empty — otherwise `NotConfigured`.
/// 2. File must be readable — otherwise `Invalid`.
/// 3. Contents must be valid JSON — otherwise `Invalid`.
/// 4. `schema_version` must equal `"promoted-v1"` — otherwise `Invalid`.
/// 5. `artifact_id`, `artifact_type`, `stage`, `produced_by` must be
///    present and non-empty strings — otherwise `Invalid`.
///
/// On success, returns `Accepted` with all four required fields.
pub fn evaluate_artifact_intake(path: Option<&Path>) -> ArtifactIntakeOutcome {
    let path = match path {
        None => return ArtifactIntakeOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return ArtifactIntakeOutcome::NotConfigured,
        Some(p) => p,
    };

    // Step 1: read file.
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return ArtifactIntakeOutcome::Invalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    // Step 2: parse JSON.
    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return ArtifactIntakeOutcome::Invalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    // Step 3: validate schema_version.
    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == PROMOTED_ARTIFACT_SCHEMA_VERSION => {}
        Some(other) => {
            return ArtifactIntakeOutcome::Invalid {
                reason: format!(
                    "unsupported schema_version '{}'; expected '{}'",
                    other, PROMOTED_ARTIFACT_SCHEMA_VERSION
                ),
            }
        }
        None => {
            return ArtifactIntakeOutcome::Invalid {
                reason: "missing 'schema_version' field".to_string(),
            }
        }
    }

    // Step 4: extract and validate required identity fields.
    let artifact_id = match required_str_field(&j, "artifact_id") {
        Ok(s) => s,
        Err(reason) => return ArtifactIntakeOutcome::Invalid { reason },
    };
    let artifact_type = match required_str_field(&j, "artifact_type") {
        Ok(s) => s,
        Err(reason) => return ArtifactIntakeOutcome::Invalid { reason },
    };
    let stage = match required_str_field(&j, "stage") {
        Ok(s) => s,
        Err(reason) => return ArtifactIntakeOutcome::Invalid { reason },
    };
    let produced_by = match required_str_field(&j, "produced_by") {
        Ok(s) => s,
        Err(reason) => return ArtifactIntakeOutcome::Invalid { reason },
    };

    ArtifactIntakeOutcome::Accepted {
        artifact_id,
        artifact_type,
        stage,
        produced_by,
    }
}

/// Extract a required non-empty string field from a JSON object.
fn required_str_field(j: &serde_json::Value, field: &str) -> Result<String, String> {
    match j.get(field).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
        Some(_) => Err(format!("field '{field}' is present but empty")),
        None => Err(format!("missing required field '{field}'")),
    }
}

// ---------------------------------------------------------------------------
// Production entry point (reads env var)
// ---------------------------------------------------------------------------

/// Read [`ENV_ARTIFACT_PATH`] from the environment and evaluate artifact intake.
///
/// Returns `NotConfigured` when the env var is absent or empty.
/// Delegates all validation to [`evaluate_artifact_intake`].
pub fn evaluate_artifact_intake_from_env() -> ArtifactIntakeOutcome {
    let raw = std::env::var(ENV_ARTIFACT_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_artifact_intake(path.as_deref())
}

/// Env var that forces the guarded evaluator to return `Unavailable` in debug
/// builds, allowing integration tests to directly exercise the mounted route's
/// `unavailable` branch without triggering a real panic.
///
/// Only checked when `debug_assertions` is enabled (debug builds / tests).
/// Compiled out entirely in release builds.
pub const ENV_FORCE_UNAVAILABLE_FOR_TEST: &str = "MQK_ARTIFACT_INTAKE_FORCE_UNAVAILABLE";

/// Panic-safe wrapper: run [`evaluate_artifact_intake_from_env`] and surface
/// any unexpected panic as [`ArtifactIntakeOutcome::Unavailable`].
///
/// This is the entry point used by the mounted route handler.  It ensures the
/// route always returns a structured response — even if the evaluator panics
/// due to an unexpected infrastructure failure — rather than crashing the
/// request handler.
///
/// Under normal operation the evaluator is deterministic and does not panic;
/// the `Unavailable` branch is a fail-closed safety net for future evaluation
/// complexity (e.g., DB-backed artifact registries).
///
/// # Test seam
/// In debug builds, setting `MQK_ARTIFACT_INTAKE_FORCE_UNAVAILABLE=1` forces
/// this function to return `Unavailable` immediately, allowing integration
/// tests to exercise the mounted route's `unavailable` branch end-to-end
/// without triggering a real evaluator panic.
pub fn evaluate_artifact_intake_guarded() -> ArtifactIntakeOutcome {
    // Debug-only test seam: allow integration tests to force the Unavailable
    // branch via env var.  Compiled out in release builds.
    #[cfg(debug_assertions)]
    if std::env::var(ENV_FORCE_UNAVAILABLE_FOR_TEST)
        .unwrap_or_default()
        .trim()
        == "1"
    {
        return ArtifactIntakeOutcome::Unavailable {
            reason: format!(
                "test-forced unavailable via {}=1",
                ENV_FORCE_UNAVAILABLE_FOR_TEST
            ),
        };
    }

    match std::panic::catch_unwind(evaluate_artifact_intake_from_env) {
        Ok(outcome) => outcome,
        Err(_) => ArtifactIntakeOutcome::Unavailable {
            reason: "artifact intake evaluator panicked unexpectedly".to_string(),
        },
    }
}
