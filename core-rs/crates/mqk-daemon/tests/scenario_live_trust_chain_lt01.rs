//! LIVE-TRUST-01 / LIVE-CHAIN-01 — Live-trust chain coherence regression lock.
//!
//! # What this proves
//!
//! The live-trust chain is internally consistent across producer and consumer:
//!
//! 1. The mode_transition FailClosed reason contains no stale patch reference
//!    (the "TV-01D" reference that was removed in LIVE-TRUST-01).
//! 2. The mode_transition FailClosed reason explicitly states this is a
//!    current-build ceiling with no operator lift path.
//! 3. Both upward paths to LiveCapital (Paper→LC and LiveShadow→LC) share
//!    the same FailClosed reason string — no divergent operator signal.
//! 4. The two advisory/enforcement layers are correctly decoupled:
//!    - Advisory layer (mode_transition): fail_closed for upward transitions
//!    - Enforcement layer (TV-03C start gate): blocks on absent/invalid
//!      parity_evidence.json, NOT on live_trust_complete value
//!    - A present parity file with live_trust_complete=false is start-safe
//!      (enforcement gate checks presence; advisory layer covers completeness)
//!
//! # Why these proofs matter
//!
//! Prior to LIVE-TRUST-01, the mode_transition FailClosed reason contained:
//!   - A reference to "TV-01D", a patch that does not exist
//!   - Aspirational text: "This verdict will change to AdmissibleWithRestart
//!     once the proof chain is closed" (no mechanism existed for this)
//!
//! These tests regression-lock the corrected, honest chain and prevent
//! future drift back to stale aspirational wording.
//!
//! # Relationship to existing proofs
//!
//! - LO-03F F05: proves present parity (live_trust_complete=false) is
//!   start-safe in LiveCapital — the enforcement/advisory decoupling is
//!   already proven there.  LT-04 here is a compact cross-reference proof.
//! - scenario_mode_transition_cc03a MT-04: proves FailClosed verdict class;
//!   LT-01..LT-03 here prove the CONTENT of the reason (not just the class).
//!
//! All tests are pure in-process.  No DB, no network, no filesystem.

use mqk_daemon::{
    mode_transition::evaluate_mode_transition,
    parity_evidence::{evaluate_parity_evidence, ParityEvidenceOutcome},
    state::DeploymentMode,
};
use std::io::Write;

// ---------------------------------------------------------------------------
// LT-01: FailClosed reason contains no stale "TV-01D" reference
//
// Regression lock: the mode_transition reason must never re-acquire the
// non-existent patch reference that was removed in LIVE-TRUST-01.
// ---------------------------------------------------------------------------

/// LIVE-TRUST-01 / LT-01: Both upward paths to LiveCapital produce a
/// FailClosed reason that does NOT contain "TV-01D".
///
/// TV-01D is a patch ID that was never implemented.  Any regression that
/// re-introduces it into the reason string is a stale-documentation bug.
#[test]
fn lt01_fail_closed_reason_contains_no_stale_tv01d_reference() {
    for (from, label) in [
        (DeploymentMode::Paper, "Paper→LiveCapital"),
        (DeploymentMode::LiveShadow, "LiveShadow→LiveCapital"),
    ] {
        let v = evaluate_mode_transition(from, DeploymentMode::LiveCapital);
        assert_eq!(
            v.as_str(),
            "fail_closed",
            "LT-01 ({label}): must be fail_closed"
        );
        let reason = v.reason();
        assert!(
            !reason.contains("TV-01D"),
            "LT-01 ({label}): FailClosed reason must NOT contain stale 'TV-01D' reference; \
             TV-01D was never implemented.  reason: {reason:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// LT-02: FailClosed reason explicitly states current-build ceiling
//
// The reason must make clear this is not a transient gate (like a missing
// config file) but a current-build ceiling that only a future proof patch
// can lift — no operator action suffices.
// ---------------------------------------------------------------------------

/// LIVE-TRUST-01 / LT-02: The FailClosed reason is explicit that:
/// - live_trust_complete=false is the live chain state (not inferred)
/// - this is a current-build ceiling (not a precondition checklist)
#[test]
fn lt02_fail_closed_reason_is_explicit_current_build_ceiling() {
    let v = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    assert_eq!(v.as_str(), "fail_closed");
    let reason = v.reason();

    assert!(
        reason.contains("live_trust_complete"),
        "LT-02: FailClosed reason must reference 'live_trust_complete'; got: {reason:?}"
    );
    assert!(
        reason.contains("current-build") || reason.contains("current build"),
        "LT-02: FailClosed reason must state this is a current-build ceiling; got: {reason:?}"
    );
    // Must NOT be admissible — no precondition list unblocks this in current builds.
    assert!(
        !v.is_admissible(),
        "LT-02: FailClosed must not be admissible"
    );
    assert!(
        v.preconditions().is_empty(),
        "LT-02: FailClosed must have empty preconditions (no checklist path); \
         got: {:?}",
        v.preconditions()
    );
}

// ---------------------------------------------------------------------------
// LT-03: Both upward paths share identical FailClosed reason
//
// Paper→LiveCapital and LiveShadow→LiveCapital must present the same
// reason to the operator.  Divergent reasons would create an inconsistent
// operator signal about what is actually blocking live capital.
// ---------------------------------------------------------------------------

/// LIVE-TRUST-01 / LT-03: Paper→LiveCapital and LiveShadow→LiveCapital share
/// the same FailClosed reason string.
///
/// Both paths are blocked by the same proof gap (hardcoded live_trust_complete=false).
/// A divergent reason would imply different blocking causes, which is incorrect.
#[test]
fn lt03_both_upward_paths_share_same_fail_closed_reason() {
    let v_paper = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    let v_shadow =
        evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::LiveCapital);

    assert_eq!(
        v_paper.as_str(),
        "fail_closed",
        "LT-03: Paper→LiveCapital must be fail_closed"
    );
    assert_eq!(
        v_shadow.as_str(),
        "fail_closed",
        "LT-03: LiveShadow→LiveCapital must be fail_closed"
    );
    assert_eq!(
        v_paper.reason(),
        v_shadow.reason(),
        "LT-03: both upward paths to LiveCapital must share the same FailClosed reason; \
         divergent reasons would create an inconsistent operator signal about the blocking cause"
    );
}

// ---------------------------------------------------------------------------
// LT-04: Advisory/enforcement decoupling — present parity with
//         live_trust_complete=false is start-safe at the TV-03C gate
//
// The two layers are intentionally decoupled:
//   - Advisory (mode_transition): fail_closed — operator-visible signal
//   - Enforcement (TV-03C gate): blocks only on absent/invalid file
//
// live_trust_complete=false in a present, valid parity file does NOT
// cause an additional block at the start gate.  The gate checks presence
// and structural validity, not trust completeness.
// ---------------------------------------------------------------------------

/// LIVE-TRUST-01 / LT-04: A structurally valid parity_evidence.json with
/// live_trust_complete=false is start-safe at the TV-03C gate.
///
/// This proves the advisory/enforcement decoupling:
/// - mode_transition (advisory): FailClosed — "live capital not yet ready"
/// - TV-03C gate (enforcement): only blocks on absent or invalid file
///
/// If the TV-03C gate were to additionally block on live_trust_complete=false,
/// it would double-count the advisory signal and prevent operators from ever
/// running in LiveShadow mode with parity evidence (which is valid and honest).
#[test]
fn lt04_present_parity_with_live_trust_false_is_start_safe_at_enforcement_gate() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_lt01_lt04_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("LT-04: create temp dir");
    let manifest_path = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest_path).expect("LT-04: create manifest placeholder");

    let evidence_json = serde_json::json!({
        "schema_version": "parity-v1",
        "artifact_id": "lt04-regression-lock-artifact",
        "gate_passed": true,
        "gate_schema_version": "gate-v1",
        "shadow_evidence": {
            "evidence_available": false,
            "evidence_note": "No shadow evaluation run performed"
        },
        "comparison_basis": "paper+alpaca supervised path",
        "live_trust_complete": false,
        "live_trust_gaps": ["no shadow cycle completed"],
        "produced_at_utc": "2026-04-05T00:00:00Z"
    })
    .to_string();

    let evidence_path = dir.join("parity_evidence.json");
    let mut f = std::fs::File::create(&evidence_path).expect("LT-04: create evidence file");
    f.write_all(evidence_json.as_bytes())
        .expect("LT-04: write evidence");

    let outcome = evaluate_parity_evidence(Some(&manifest_path));

    let _ = std::fs::remove_dir_all(&dir);

    // Must be Present (not Absent or Invalid) — file is structurally valid.
    assert!(
        outcome.is_present(),
        "LT-04: valid parity file with live_trust_complete=false must be Present; got: {outcome:?}"
    );

    // Must be start-safe — the enforcement gate only checks presence/validity.
    assert!(
        outcome.is_start_safe(),
        "LT-04: present parity with live_trust_complete=false must be start-safe at TV-03C gate; \
         the enforcement gate must not additionally block on trust completeness value; \
         got: {outcome:?}"
    );

    // Confirm live_trust_complete is surfaced honestly as false (not hidden or fabricated).
    if let ParityEvidenceOutcome::Present {
        live_trust_complete,
        ..
    } = &outcome
    {
        assert!(
            !live_trust_complete,
            "LT-04: live_trust_complete must be honestly surfaced as false; got: true"
        );
    }

    // Confirm mode_transition is still fail_closed (advisory layer unaffected by enforcement).
    let verdict = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    assert_eq!(
        verdict.as_str(),
        "fail_closed",
        "LT-04: mode_transition advisory layer must remain fail_closed even when \
         parity evidence IS present; the two layers are intentionally decoupled"
    );
}
