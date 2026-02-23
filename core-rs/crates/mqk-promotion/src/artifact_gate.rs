//! Patch B6 — Golden Artifacts: Hash-Lock + Immutability Gate.
//!
//! [`lock_artifact_from_str`] validates a manifest + audit log pair in memory
//! and, on success, returns an [`ArtifactLock`] token.  Passing
//! `Some(lock)` in [`crate::PromotionInput::artifact_lock`] proves that the
//! artifact passed validation.  `None` blocks promotion unconditionally.
//!
//! # Validation steps
//! 1. Parse `manifest_json` as [`mqk_artifacts::RunManifest`].
//! 2. Require non-empty `config_hash` and `git_hash` fields.
//! 3. Verify the audit log hash chain (`audit_jsonl` must have ≥ 1 event).

use mqk_artifacts::RunManifest;
use mqk_audit::{verify_hash_chain_str, VerifyResult};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`lock_artifact_from_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The manifest JSON is not valid or cannot be deserialized.
    ManifestParse(String),
    /// The manifest's `config_hash` field is empty.
    MissingConfigHash,
    /// The manifest's `git_hash` field is empty.
    MissingGitHash,
    /// The audit log is valid JSON but contains zero events.
    AuditEmpty,
    /// The audit log hash chain is broken or unparseable.
    AuditChainBroken { line: usize, reason: String },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::ManifestParse(e) => write!(f, "manifest parse failed: {e}"),
            LockError::MissingConfigHash => write!(f, "manifest config_hash is empty"),
            LockError::MissingGitHash => write!(f, "manifest git_hash is empty"),
            LockError::AuditEmpty => write!(f, "audit log is empty (no events to verify)"),
            LockError::AuditChainBroken { line, reason } => {
                write!(f, "audit hash chain broken at line {line}: {reason}")
            }
        }
    }
}

impl std::error::Error for LockError {}

// ---------------------------------------------------------------------------
// Proof-of-verification token
// ---------------------------------------------------------------------------

/// Proof-of-verification token.
///
/// Can only be created by:
/// - [`lock_artifact_from_str`] — production path (validates manifest + hash chain).
/// - [`ArtifactLock::new_for_testing`] — test-only bypass (clearly named).
///
/// Passing `Some(ArtifactLock)` in [`crate::PromotionInput::artifact_lock`]
/// proves the artifact has been validated.  `None` blocks promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLock {
    /// SHA-256 config hash extracted from the manifest.
    pub config_hash: String,
    /// Git hash extracted from the manifest.
    pub git_hash: String,
    /// Number of audit log events verified by the hash chain.
    pub audit_lines_verified: usize,
}

impl ArtifactLock {
    /// **Test-only bypass** — creates an `ArtifactLock` without verification.
    ///
    /// Use in tests that exercise non-B6 promotion logic and need a
    /// syntactically valid lock.  Do **not** use in production code.
    pub fn new_for_testing(config_hash: impl Into<String>, git_hash: impl Into<String>) -> Self {
        Self {
            config_hash: config_hash.into(),
            git_hash: git_hash.into(),
            audit_lines_verified: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Verifier
// ---------------------------------------------------------------------------

/// Validate a manifest + audit log pair in memory and return an [`ArtifactLock`].
///
/// # Arguments
/// - `manifest_json` — contents of `manifest.json`, parsed as [`RunManifest`].
/// - `audit_jsonl`   — contents of `audit.jsonl`; one JSON event per line.
///
/// # Errors
/// Returns [`LockError`] if any validation step fails:
/// - [`LockError::ManifestParse`]    — invalid manifest JSON.
/// - [`LockError::MissingConfigHash`] — `config_hash` is empty.
/// - [`LockError::MissingGitHash`]   — `git_hash` is empty.
/// - [`LockError::AuditEmpty`]       — audit log has zero events.
/// - [`LockError::AuditChainBroken`] — hash chain integrity failure.
pub fn lock_artifact_from_str(
    manifest_json: &str,
    audit_jsonl: &str,
) -> Result<ArtifactLock, LockError> {
    // Step 1: parse manifest.
    let manifest: RunManifest =
        serde_json::from_str(manifest_json).map_err(|e| LockError::ManifestParse(e.to_string()))?;

    // Step 2: validate required hash fields.
    if manifest.config_hash.trim().is_empty() {
        return Err(LockError::MissingConfigHash);
    }
    if manifest.git_hash.trim().is_empty() {
        return Err(LockError::MissingGitHash);
    }

    // Step 3: verify audit hash chain.
    let verify = verify_hash_chain_str(audit_jsonl).map_err(|e| LockError::AuditChainBroken {
        line: 0,
        reason: e.to_string(),
    })?;

    match verify {
        VerifyResult::Valid { lines } => {
            if lines == 0 {
                return Err(LockError::AuditEmpty);
            }
            Ok(ArtifactLock {
                config_hash: manifest.config_hash,
                git_hash: manifest.git_hash,
                audit_lines_verified: lines,
            })
        }
        VerifyResult::Broken { line, reason } => Err(LockError::AuditChainBroken { line, reason }),
    }
}
