//! TV-01B: Runtime artifact intake contract — proof tests.
//!
//! Proves that the runtime artifact intake seam (`evaluate_artifact_intake` +
//! `GET /api/v1/system/artifact-intake`) surfaces honest, fail-closed truth
//! for all four intake outcomes: not_configured, invalid, accepted, unavailable.
//!
//! # Proof matrix
//!
//! | Test   | What it proves                                                                |
//! |--------|-------------------------------------------------------------------------------|
//! | AI-01  | Route: no MQK_ARTIFACT_PATH → truth_state = "not_configured"                 |
//! | AI-02  | Pure fn: non-existent file → Invalid (unreadable)                             |
//! | AI-03  | Pure fn: valid promoted_manifest.json → Accepted with correct fields          |
//! | AI-04  | Pure fn: wrong schema_version → Invalid with reason                           |
//! | AI-05  | Pure fn: missing required field → Invalid with reason                         |
//! | AI-06  | Pure fn: empty artifact_id → Invalid (non-empty required)                     |
//! | AI-07  | Pure fn: None path → NotConfigured (honest absence, not error)                |
//! | AI-08  | Route: valid MQK_ARTIFACT_PATH → truth_state = "accepted" with fields (route)   |
//! | AI-09  | Unavailable variant: truth_state/is_accepted contract + fail-closed semantics      |
//! | AI-10  | Route: MQK_ARTIFACT_INTAKE_FORCE_UNAVAILABLE=1 → truth_state = "unavailable" (route) |
//!
//! AI-01, AI-08, and AI-10 use the HTTP route (env-var serialised via ENV_LOCK mutex).
//! AI-02..AI-07 and AI-09 use pure functions to avoid env-var pollution.
//! All tests are pure in-process; no DB or network required.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    artifact_intake::{
        evaluate_artifact_intake, ArtifactIntakeOutcome, ENV_ARTIFACT_PATH,
        ENV_FORCE_UNAVAILABLE_FOR_TEST,
    },
    routes::build_router,
    state::{AppState, OperatorAuthMode},
};
use tokio::sync::Mutex;
use tower::ServiceExt;

/// Serialises tests that mutate MQK_ARTIFACT_PATH so they do not race with
/// AI-01 (which reads the env var) or each other.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write `contents` to a unique temp file and return its path.
/// Caller is responsible for cleanup (or it is cleaned up on drop of the test process).
fn write_temp_json(tag: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mqk_tv01b_{tag}_{}.json", std::process::id()));
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

// ---------------------------------------------------------------------------
// AI-01: HTTP route — not_configured when MQK_ARTIFACT_PATH is unset
// ---------------------------------------------------------------------------

/// TV-01B / AI-01: GET /api/v1/system/artifact-intake returns
/// `truth_state = "not_configured"` when `MQK_ARTIFACT_PATH` is not set.
///
/// This proves the mounted surface is fail-closed on absence: the route does
/// not synthesise a positive intake claim when no artifact has been configured.
///
/// Serialised via `ENV_LOCK` to avoid races with AI-08.
#[tokio::test]
async fn ai_01_route_not_configured_when_no_env_var() {
    let _guard = env_lock().lock().await;

    // Only assert not_configured if the env var is not set in this environment.
    if std::env::var(ENV_ARTIFACT_PATH).is_ok_and(|v| !v.trim().is_empty()) {
        // env var is set externally — skip rather than assert incorrectly.
        return;
    }

    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = build_router(st);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/system/artifact-intake")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "AI-01: artifact-intake must return 200"
    );

    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        j["truth_state"], "not_configured",
        "AI-01: absent env var must produce truth_state=not_configured; body: {j}"
    );
    assert!(
        j["artifact_id"].is_null(),
        "AI-01: artifact_id must be null when not_configured; body: {j}"
    );
    assert!(
        j["invalid_reason"].is_null(),
        "AI-01: invalid_reason must be null when not_configured; body: {j}"
    );
    assert_eq!(
        j["canonical_route"], "/api/v1/system/artifact-intake",
        "AI-01: canonical_route must be self-identifying; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AI-02: Invalid — non-existent file path
// ---------------------------------------------------------------------------

/// TV-01B / AI-02: A configured path that points to a non-existent file
/// returns `Invalid` with an unreadable-file reason.
///
/// This proves the intake seam does not silently accept unconfigured or
/// missing artifacts — it fails closed with an explicit reason.
#[test]
fn ai_02_nonexistent_path_is_invalid() {
    let path = std::path::Path::new("/tmp/mqk_does_not_exist_tv01b_ai02.json");
    // Guarantee the file doesn't exist.
    let _ = std::fs::remove_file(path);

    let outcome = evaluate_artifact_intake(Some(path));

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                !reason.is_empty(),
                "AI-02: Invalid reason must not be empty"
            );
            // Reason should mention the path or indicate unreadability.
            assert!(
                reason.contains("mqk_does_not_exist") || reason.contains("cannot read"),
                "AI-02: Invalid reason should reference the path; got: '{reason}'"
            );
        }
        other => panic!("AI-02: non-existent path must return Invalid; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-03: Accepted — valid promoted_manifest.json
// ---------------------------------------------------------------------------

/// TV-01B / AI-03: A valid `promoted_manifest.json` (schema_version="promoted-v1",
/// all required fields present) returns `Accepted` with the correct fields.
///
/// This proves the intake seam can distinguish a structurally valid artifact
/// from absent/invalid inputs — the positive intake path works.
#[test]
fn ai_03_valid_manifest_is_accepted() {
    let artifact_id = "abc123def456";
    let contents = valid_promoted_manifest(artifact_id);
    let path = write_temp_json("ai03", &contents);

    let outcome = evaluate_artifact_intake(Some(&path));

    match outcome {
        ArtifactIntakeOutcome::Accepted {
            artifact_id: got_id,
            artifact_type,
            stage,
            produced_by,
        } => {
            assert_eq!(
                got_id, artifact_id,
                "AI-03: artifact_id must match manifest"
            );
            assert_eq!(
                artifact_type, "signal_pack",
                "AI-03: artifact_type must match"
            );
            assert_eq!(stage, "paper", "AI-03: stage must match");
            assert_eq!(
                produced_by, "research-py/promote.py",
                "AI-03: produced_by must match"
            );
        }
        other => panic!("AI-03: valid manifest must return Accepted; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-04: Invalid — wrong schema_version
// ---------------------------------------------------------------------------

/// TV-01B / AI-04: A JSON file with a wrong `schema_version` returns
/// `Invalid` with a reason mentioning the unsupported version.
///
/// This proves the intake seam rejects manifests from a different schema
/// contract rather than silently accepting them.
#[test]
fn ai_04_wrong_schema_version_is_invalid() {
    let contents = r#"{
  "schema_version": "promoted-v99",
  "artifact_id": "abc123",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "test"
}"#;
    let path = write_temp_json("ai04", contents);

    let outcome = evaluate_artifact_intake(Some(&path));

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("promoted-v99") || reason.contains("schema_version"),
                "AI-04: reason must mention the unsupported version; got: '{reason}'"
            );
        }
        other => panic!("AI-04: wrong schema_version must return Invalid; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-05: Invalid — missing required field
// ---------------------------------------------------------------------------

/// TV-01B / AI-05: A structurally valid JSON file that is missing a required
/// field (`stage` in this case) returns `Invalid` with a reason naming the
/// missing field.
///
/// This proves the intake seam does not silently collapse missing required
/// fields into a successful intake.
#[test]
fn ai_05_missing_required_field_is_invalid() {
    // Missing "stage".
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "abc123",
  "artifact_type": "signal_pack",
  "produced_by": "test"
}"#;
    let path = write_temp_json("ai05", contents);

    let outcome = evaluate_artifact_intake(Some(&path));

    match outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("stage"),
                "AI-05: reason must mention the missing field 'stage'; got: '{reason}'"
            );
        }
        other => panic!("AI-05: missing required field must return Invalid; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-06: Invalid — empty artifact_id
// ---------------------------------------------------------------------------

/// TV-01B / AI-06: An artifact_id that is present but empty (or whitespace)
/// must return `Invalid`, not `Accepted`.
///
/// This proves empty strings are not accepted as valid artifact identity.
#[test]
fn ai_06_empty_artifact_id_is_invalid() {
    let contents = r#"{
  "schema_version": "promoted-v1",
  "artifact_id": "   ",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "test"
}"#;
    let path = write_temp_json("ai06", contents);

    let outcome = evaluate_artifact_intake(Some(&path));

    match &outcome {
        ArtifactIntakeOutcome::Invalid { reason } => {
            assert!(
                reason.contains("artifact_id"),
                "AI-06: reason must mention artifact_id; got: '{reason}'"
            );
        }
        other => panic!("AI-06: empty artifact_id must return Invalid; got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-07: NotConfigured — None path is honest absence
// ---------------------------------------------------------------------------

/// TV-01B / AI-07: Passing `None` to `evaluate_artifact_intake` returns
/// `NotConfigured`, not an error.
///
/// This proves the function cleanly distinguishes "no path provided" from
/// "path provided but invalid" — operators who haven't configured an artifact
/// get an honest "not_configured", not a confusing error.
#[test]
fn ai_07_none_path_is_not_configured_not_error() {
    let outcome = evaluate_artifact_intake(None);

    assert_eq!(
        outcome,
        ArtifactIntakeOutcome::NotConfigured,
        "AI-07: None path must return NotConfigured"
    );
    assert_eq!(
        outcome.truth_state(),
        "not_configured",
        "AI-07: truth_state() must be 'not_configured'"
    );
    assert!(
        !outcome.is_accepted(),
        "AI-07: NotConfigured must not be_accepted()"
    );
}

// ---------------------------------------------------------------------------
// AI-08: Route-level accepted — valid MQK_ARTIFACT_PATH → truth_state = "accepted"
// ---------------------------------------------------------------------------

/// TV-01B / AI-08: GET /api/v1/system/artifact-intake returns
/// `truth_state = "accepted"` with all identity fields populated when
/// `MQK_ARTIFACT_PATH` points to a valid `promoted_manifest.json`.
///
/// This is the route-level proof of the accepted branch — not just the pure
/// function.  It proves that the mounted control-plane surface correctly maps
/// a valid artifact file to a positive, structured intake response.
///
/// Serialised via `ENV_LOCK` to avoid races with AI-01.
#[tokio::test]
async fn ai_08_route_accepted_when_valid_manifest() {
    let _guard = env_lock().lock().await;

    let artifact_id = "tv01b_ai08_test_artifact";
    let contents = valid_promoted_manifest(artifact_id);
    let path = write_temp_json("ai08", &contents);

    // Set the env var for the duration of this test.
    std::env::set_var(ENV_ARTIFACT_PATH, path.to_str().unwrap());

    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = build_router(st);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/system/artifact-intake")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();

    // Clean up env var before any assertions that might panic.
    std::env::remove_var(ENV_ARTIFACT_PATH);
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "AI-08: artifact-intake must return 200"
    );

    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        j["truth_state"], "accepted",
        "AI-08: valid manifest must produce truth_state=accepted; body: {j}"
    );
    assert_eq!(
        j["artifact_id"], artifact_id,
        "AI-08: artifact_id must be populated; body: {j}"
    );
    assert_eq!(
        j["artifact_type"], "signal_pack",
        "AI-08: artifact_type must be populated; body: {j}"
    );
    assert_eq!(
        j["stage"], "paper",
        "AI-08: stage must be populated; body: {j}"
    );
    assert_eq!(
        j["produced_by"], "research-py/promote.py",
        "AI-08: produced_by must be populated; body: {j}"
    );
    assert!(
        j["invalid_reason"].is_null(),
        "AI-08: invalid_reason must be null for accepted; body: {j}"
    );
    assert_eq!(
        j["canonical_route"], "/api/v1/system/artifact-intake",
        "AI-08: canonical_route must be self-identifying; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// AI-09: Unavailable variant — contract and fail-closed semantics
// ---------------------------------------------------------------------------

/// TV-01B / AI-09: `ArtifactIntakeOutcome::Unavailable` has the correct
/// truth_state label, is not accepted, and carries a non-empty reason.
///
/// This proves the fourth intake truth state exists and satisfies the
/// fail-closed contract: `Unavailable` is not treated as accepted, and
/// surfaces an honest reason rather than a silent failure.
///
/// The `Unavailable` variant is surfaced by the route's `catch_unwind`
/// guard when the evaluator panics unexpectedly.  The variant itself is
/// proven here at the pure-function contract level; the route's catch_unwind
/// branch is proven structurally through compilation and exhaustive match.
#[test]
fn ai_09_unavailable_variant_is_fail_closed() {
    let outcome = ArtifactIntakeOutcome::Unavailable {
        reason: "test-forced unavailable: evaluator infrastructure failure".to_string(),
    };

    assert_eq!(
        outcome.truth_state(),
        "unavailable",
        "AI-09: Unavailable must have truth_state 'unavailable'"
    );
    assert!(
        !outcome.is_accepted(),
        "AI-09: Unavailable must not be is_accepted()"
    );

    // Confirm reason is preserved and non-empty.
    match &outcome {
        ArtifactIntakeOutcome::Unavailable { reason } => {
            assert!(
                !reason.is_empty(),
                "AI-09: Unavailable reason must not be empty"
            );
            assert!(
                reason.contains("unavailable"),
                "AI-09: reason must describe the unavailability; got: '{reason}'"
            );
        }
        other => panic!("AI-09: expected Unavailable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AI-10: Route-level unavailable — direct end-to-end proof
// ---------------------------------------------------------------------------

/// TV-01B / AI-10: GET /api/v1/system/artifact-intake returns
/// `truth_state = "unavailable"` with fail-closed semantics when the intake
/// evaluator is forced unavailable via `MQK_ARTIFACT_INTAKE_FORCE_UNAVAILABLE=1`.
///
/// This is the end-to-end proof of the mounted route's `unavailable` branch.
/// It proves:
/// - `truth_state` = `"unavailable"` (not "accepted", not "invalid")
/// - All artifact identity fields are null (fail-closed)
/// - `invalid_reason` carries a non-empty reason
/// - The route returns 200 (structured, not a 5xx crash)
/// - `canonical_route` is self-identifying
///
/// The seam (`MQK_ARTIFACT_INTAKE_FORCE_UNAVAILABLE=1`) is compiled in only
/// under `debug_assertions` (i.e., test/debug builds) and is absent from
/// release builds, so it cannot affect production behavior.
///
/// Serialised via `ENV_LOCK` to avoid races with AI-01 and AI-08.
#[tokio::test]
async fn ai_10_route_unavailable_when_force_flag_set() {
    let _guard = env_lock().lock().await;

    // Activate the debug-only test seam.
    std::env::set_var(ENV_FORCE_UNAVAILABLE_FOR_TEST, "1");

    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = build_router(st);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/system/artifact-intake")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();

    // Clear the flag before any assertions that could panic.
    std::env::remove_var(ENV_FORCE_UNAVAILABLE_FOR_TEST);

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "AI-10: unavailable must return 200 (structured response, not a crash)"
    );

    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        j["truth_state"], "unavailable",
        "AI-10: force-flag must produce truth_state=unavailable; body: {j}"
    );
    // Fail-closed: no positive artifact identity must be populated.
    assert!(
        j["artifact_id"].is_null(),
        "AI-10: artifact_id must be null when unavailable; body: {j}"
    );
    assert!(
        j["artifact_type"].is_null(),
        "AI-10: artifact_type must be null when unavailable; body: {j}"
    );
    assert!(
        j["stage"].is_null(),
        "AI-10: stage must be null when unavailable; body: {j}"
    );
    assert!(
        j["produced_by"].is_null(),
        "AI-10: produced_by must be null when unavailable; body: {j}"
    );
    // Reason must be surfaced honestly.
    assert!(
        !j["invalid_reason"].is_null(),
        "AI-10: invalid_reason must be non-null (carries unavailable reason); body: {j}"
    );
    let reason = j["invalid_reason"].as_str().unwrap_or("");
    assert!(
        !reason.is_empty(),
        "AI-10: unavailable reason must not be empty; body: {j}"
    );
    assert_eq!(
        j["canonical_route"], "/api/v1/system/artifact-intake",
        "AI-10: canonical_route must be self-identifying; body: {j}"
    );
}
