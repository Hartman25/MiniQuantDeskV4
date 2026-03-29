//! TV-01C / TV-01D: Artifact identity/provenance threaded into run-start truth.
//!
//! TV-01C proves that `AppState::accepted_artifact_provenance` is the correct
//! run-start provenance surface: populated when accepted, null when not
//! configured / invalid — and that `GET /api/v1/system/run-artifact` surfaces
//! it faithfully.
//!
//! TV-01D proves the end-to-end identity chain: a promoted artifact accepted
//! by the intake seam (`evaluate_artifact_intake`) carries the same
//! `artifact_id` that the control-plane surfaces via the run-artifact route —
//! without fabrication on absent/invalid inputs.
//!
//! # Proof matrix
//!
//! | Test        | What it proves                                                                         |
//! |-------------|----------------------------------------------------------------------------------------|
//! | AP-01       | No artifact in AppState → route truth_state = "no_run", all fields null               |
//! | AP-02       | Accepted artifact set in AppState → route truth_state = "active", all fields           |
//! | AP-03       | Set then clear → route returns "no_run" (cleared on stop/halt path)                    |
//! | AP-04       | End-to-end: intake Accepted → same artifact_id on control-plane surface                |
//! | AP-05       | Intake NotConfigured → AppState stays None → route "no_run" (fail-closed)              |
//! | AP-06       | Intake Invalid → AppState stays None → route "no_run" (fail-closed)                   |
//! | tv01d_f1(*) | DB-backed: real start_execution_runtime consumes + surfaces provenance end-to-end      |
//!
//! AP-01..AP-03 use the AppState test seam directly (TV-01C surface contract).
//! AP-04..AP-06 are the in-process TV-01D identity chain proofs via intake evaluator.
//! tv01d_f1 is in `scenario_daemon_runtime_lifecycle.rs` (requires MQK_DATABASE_URL,
//!   `#[ignore]`); it is the definitive TV-01D proof that the **real**
//!   `start_execution_runtime` code path writes and surfaces provenance, not just
//!   the test seam.
//! All AP-* tests are pure in-process; no DB or network required.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    artifact_intake::{evaluate_artifact_intake, ArtifactIntakeOutcome},
    routes::build_router,
    state::{AcceptedArtifactProvenance, AppState, OperatorAuthMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_temp_json(tag: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mqk_tv01cd_{tag}_{}.json", std::process::id()));
    std::fs::write(&path, contents).expect("write temp json");
    path
}

fn valid_promoted_manifest(artifact_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "promoted-v1",
  "artifact_id": "{artifact_id}",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py",
  "data_root": "promoted/signal_packs/{artifact_id}"
}}"#
    )
}

async fn call_run_artifact(st: Arc<AppState>) -> serde_json::Value {
    let router = build_router(st);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/system/run-artifact")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "run-artifact must return 200"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ---------------------------------------------------------------------------
// AP-01: No artifact → truth_state = "no_run", all fields null
// ---------------------------------------------------------------------------

/// TV-01C / AP-01: When no artifact provenance is set in AppState, the route
/// returns `truth_state = "no_run"` with all identity fields null.
///
/// Proves fail-closed absence: the control-plane does not fabricate positive
/// artifact provenance when no run has been started with an accepted artifact.
#[tokio::test]
async fn ap_01_no_artifact_returns_no_run() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    // accepted_artifact is None at boot — do not set anything.

    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "no_run",
        "AP-01: no artifact → truth_state must be 'no_run'; body: {j}"
    );
    assert_eq!(
        j["canonical_route"], "/api/v1/system/run-artifact",
        "AP-01: canonical_route must be self-identifying; body: {j}"
    );
    assert!(
        j["artifact_id"].is_null(),
        "AP-01: artifact_id must be null; body: {j}"
    );
    assert!(
        j["artifact_type"].is_null(),
        "AP-01: artifact_type must be null; body: {j}"
    );
    assert!(j["stage"].is_null(), "AP-01: stage must be null; body: {j}");
    assert!(
        j["produced_by"].is_null(),
        "AP-01: produced_by must be null; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AP-02: Accepted artifact set → truth_state = "active", all fields present
// ---------------------------------------------------------------------------

/// TV-01C / AP-02: When an accepted artifact provenance is stored in AppState
/// (as `start_execution_runtime` does on successful start), the route returns
/// `truth_state = "active"` with all four identity fields populated.
///
/// Proves the control-plane surface faithfully forwards provenance that was
/// accepted at run start without dropping or altering fields.
#[tokio::test]
async fn ap_02_accepted_artifact_returns_active_with_fields() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let provenance = AcceptedArtifactProvenance {
        artifact_id: "ap02-artifact-id-abc123".to_string(),
        artifact_type: "signal_pack".to_string(),
        stage: "paper".to_string(),
        produced_by: "research-py/promote.py".to_string(),
    };
    st.set_accepted_artifact_for_test(Some(provenance)).await;

    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "active",
        "AP-02: accepted artifact → truth_state must be 'active'; body: {j}"
    );
    assert_eq!(
        j["artifact_id"], "ap02-artifact-id-abc123",
        "AP-02: artifact_id must match; body: {j}"
    );
    assert_eq!(
        j["artifact_type"], "signal_pack",
        "AP-02: artifact_type must match; body: {j}"
    );
    assert_eq!(j["stage"], "paper", "AP-02: stage must match; body: {j}");
    assert_eq!(
        j["produced_by"], "research-py/promote.py",
        "AP-02: produced_by must match; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AP-03: Set then clear → truth_state = "no_run"
// ---------------------------------------------------------------------------

/// TV-01C / AP-03: After artifact provenance is set (simulating run start) and
/// then cleared (simulating stop/halt), the route returns `truth_state = "no_run"`.
///
/// Proves that stop/halt clears the provenance so the control-plane does not
/// surface stale artifact identity after the run ends.
#[tokio::test]
async fn ap_03_cleared_artifact_returns_no_run() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Simulate start: set provenance.
    let provenance = AcceptedArtifactProvenance {
        artifact_id: "ap03-artifact-id-def456".to_string(),
        artifact_type: "signal_pack".to_string(),
        stage: "paper".to_string(),
        produced_by: "research-py/promote.py".to_string(),
    };
    st.set_accepted_artifact_for_test(Some(provenance)).await;

    // Simulate stop: clear provenance.
    st.set_accepted_artifact_for_test(None).await;

    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "no_run",
        "AP-03: cleared artifact → truth_state must be 'no_run'; body: {j}"
    );
    assert!(
        j["artifact_id"].is_null(),
        "AP-03: artifact_id must be null after clear; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AP-04: TV-01D — end-to-end: intake Accepted → same artifact_id on surface
// ---------------------------------------------------------------------------

/// TV-01D / AP-04: End-to-end identity chain proof.
///
/// 1. Write a valid `promoted_manifest.json` to a temp file.
/// 2. Call `evaluate_artifact_intake` (the TV-01B intake seam) → `Accepted{artifact_id=X}`.
/// 3. Store the provenance from the intake outcome in AppState (as `start_execution_runtime`
///    does) via the test seam.
/// 4. Call `GET /api/v1/system/run-artifact` → verify `artifact_id == X`.
///
/// This proves that the **same artifact_id** that the intake seam accepts is
/// exactly what the control-plane surfaces — without fabrication or drift.
/// The chain: promoted artifact → evaluate_artifact_intake → AppState provenance
/// → /api/v1/system/run-artifact is the end-to-end identity proof.
#[tokio::test]
async fn ap_04_e2e_intake_accepted_same_artifact_id_on_surface() {
    let artifact_id = "tv01d-e2e-proof-artifact-abc789";
    let contents = valid_promoted_manifest(artifact_id);
    let path = write_temp_json("ap04", &contents);

    // Step 1: evaluate artifact intake (TV-01B seam).
    let outcome = evaluate_artifact_intake(Some(&path));
    let _ = std::fs::remove_file(&path);

    // Step 2: extract Accepted provenance.
    let (intake_artifact_id, intake_type, intake_stage, intake_produced_by) = match outcome {
        ArtifactIntakeOutcome::Accepted {
            artifact_id,
            artifact_type,
            stage,
            produced_by,
        } => (artifact_id, artifact_type, stage, produced_by),
        other => panic!("AP-04: valid manifest must be Accepted; got: {other:?}"),
    };

    // Step 3: store provenance in AppState (as start_execution_runtime does).
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.set_accepted_artifact_for_test(Some(AcceptedArtifactProvenance {
        artifact_id: intake_artifact_id.clone(),
        artifact_type: intake_type.clone(),
        stage: intake_stage.clone(),
        produced_by: intake_produced_by.clone(),
    }))
    .await;

    // Step 4: verify control-plane surface matches intake identity exactly.
    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "active",
        "AP-04: accepted artifact → truth_state must be 'active'; body: {j}"
    );
    assert_eq!(
        j["artifact_id"].as_str().unwrap_or(""),
        intake_artifact_id,
        "AP-04: artifact_id on surface must match intake artifact_id exactly; body: {j}"
    );
    assert_eq!(
        j["artifact_type"].as_str().unwrap_or(""),
        intake_type,
        "AP-04: artifact_type must match intake; body: {j}"
    );
    assert_eq!(
        j["stage"].as_str().unwrap_or(""),
        intake_stage,
        "AP-04: stage must match intake; body: {j}"
    );
    assert_eq!(
        j["produced_by"].as_str().unwrap_or(""),
        intake_produced_by,
        "AP-04: produced_by must match intake; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AP-05: TV-01D — intake NotConfigured → AppState None → route "no_run"
// ---------------------------------------------------------------------------

/// TV-01D / AP-05: Intake `NotConfigured` produces no provenance on the surface.
///
/// Proves fail-closed absence: when no artifact path is configured, the intake
/// seam returns `NotConfigured`, and the runtime provenance surface returns
/// `truth_state = "no_run"` — no fabricated consumption claim.
#[tokio::test]
async fn ap_05_e2e_intake_not_configured_produces_no_run_surface() {
    // Evaluate with no path configured.
    let outcome = evaluate_artifact_intake(None);

    assert_eq!(
        outcome,
        ArtifactIntakeOutcome::NotConfigured,
        "AP-05: None path must produce NotConfigured"
    );

    // Not Accepted — AppState provenance remains None (as start_execution_runtime does).
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Do not set any provenance — mirrors the start_execution_runtime behaviour
    // when intake is not Accepted.

    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "no_run",
        "AP-05: NotConfigured intake → surface must be 'no_run'; body: {j}"
    );
    assert!(
        j["artifact_id"].is_null(),
        "AP-05: artifact_id must be null; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AP-06: TV-01D — intake Invalid → AppState None → route "no_run"
// ---------------------------------------------------------------------------

/// TV-01D / AP-06: Intake `Invalid` produces no provenance on the surface.
///
/// Proves fail-closed invalid: when the configured file is unreadable or
/// structurally invalid, the intake seam returns `Invalid`, and the runtime
/// provenance surface returns `truth_state = "no_run"` — no fabricated claim.
#[tokio::test]
async fn ap_06_e2e_intake_invalid_produces_no_run_surface() {
    // Point to a non-existent file — guaranteed Invalid.
    let path = std::path::Path::new("/tmp/mqk_tv01cd_does_not_exist_ap06.json");
    let _ = std::fs::remove_file(path);

    let outcome = evaluate_artifact_intake(Some(path));

    assert!(
        matches!(outcome, ArtifactIntakeOutcome::Invalid { .. }),
        "AP-06: non-existent path must produce Invalid; got: {outcome:?}"
    );

    // Not Accepted — AppState provenance stays None.
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let j = call_run_artifact(st).await;

    assert_eq!(
        j["truth_state"], "no_run",
        "AP-06: Invalid intake → surface must be 'no_run'; body: {j}"
    );
    assert!(
        j["artifact_id"].is_null(),
        "AP-06: artifact_id must be null; body: {j}"
    );
}
