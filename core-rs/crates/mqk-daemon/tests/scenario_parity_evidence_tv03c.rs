//! TV-03C — Start refusal when parity evidence is missing or stale.
//!
//! Proves that the parity evidence gate in `start_execution_runtime` is
//! operationally binding: absent or invalid parity evidence must refuse
//! runtime start with an explicit gate name.
//!
//! # What this proves
//!
//! - Absent parity evidence blocks start (fail-closed, gate=parity_evidence).
//! - Invalid parity evidence blocks start (fail-closed, gate=parity_evidence).
//! - Present parity evidence allows start to proceed to the next gate (DB gate).
//! - Not-configured (no artifact path) is not a parity failure: start proceeds.
//!
//! # Gate ordering context
//!
//! At the start boundary (armed LiveShadow+Alpaca, no WS continuity issue):
//!   1. deployment_readiness   → passes (LiveShadow)
//!   2. integrity              → passes (armed in setup)
//!   3. WS continuity gates    → not applicable for LiveShadow
//!   4. TV-02C artifact gate   → passes (deployability_gate.json present + passed)
//!   5. TV-03C parity gate     ← what TC-01..TC-04 exercise
//!   6. TV-04A capital policy  → not triggered (no MQK_CAPITAL_POLICY_PATH)
//!   7. db_pool()              → ServiceUnavailable (no DB in test) → 503
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                              |
//! |-------|-----------------------------------------------------------------------------|
//! | TC-01 | not_configured (no MQK_ARTIFACT_PATH) → start proceeds to DB gate (503)   |
//! | TC-02 | artifact configured + parity absent → 403 gate=parity_evidence            |
//! | TC-03 | artifact configured + parity invalid → 403 gate=parity_evidence           |
//! | TC-04 | artifact configured + parity present + deployable → proceeds to DB (503)  |
//!
//! TC-02 and TC-03 use both `promoted_manifest.json` and `deployability_gate.json`
//! so the TV-02C gate passes and the TV-03C gate is the named blocker.
//!
//! All tests require no database and no network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialisation — protects MQK_ARTIFACT_PATH mutations between tests
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn post_start() -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// State helpers
// ---------------------------------------------------------------------------

/// Build an armed LiveShadow+Alpaca AppState for start-boundary tests.
///
/// Gate ordering at start for this state:
///   1. deployment_readiness → passes (LiveShadow)
///   2. integrity            → passes (armed)
///   3. WS continuity gates  → not applicable for LiveShadow
///   4. TV-02C artifact gate → depends on MQK_ARTIFACT_PATH (set by each test)
///   5. TV-03C parity gate   ← what TC-01..TC-04 exercise
///   6. TV-04A capital gate  → not triggered (no MQK_CAPITAL_POLICY_PATH)
///   7. db_pool()            → 503 ServiceUnavailable (no DB in test)
async fn armed_live_shadow_state() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "test setup: arm must succeed");
    st
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Minimal valid `promoted_manifest.json`.
fn valid_manifest(artifact_id: &str) -> String {
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

/// Minimal valid `deployability_gate.json` with `passed=true`.
fn gate_json_passed(artifact_id: &str) -> String {
    format!(
        r#"{{
  "schema_version": "gate-v1",
  "artifact_id": "{artifact_id}",
  "passed": true,
  "checks": [],
  "overall_reason": "all checks passed",
  "evaluated_at_utc": "2026-03-01T00:00:00Z"
}}"#
    )
}

/// Minimal valid `parity_evidence.json`.
fn valid_parity_json(artifact_id: &str) -> String {
    serde_json::json!({
        "schema_version": "parity-v1",
        "artifact_id": artifact_id,
        "gate_passed": true,
        "gate_schema_version": "gate-v1",
        "shadow_evidence": {
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

/// Write a temp artifact dir with a manifest, a gate file, and optionally
/// parity evidence.  Returns `(manifest_path, dir)`.
fn write_artifact_dir(
    tag: &str,
    artifact_id: &str,
    parity_contents: Option<&str>,
) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv03c_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create artifact dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(artifact_id)).expect("write manifest");
    std::fs::write(
        dir.join("deployability_gate.json"),
        gate_json_passed(artifact_id),
    )
    .expect("write gate file");
    if let Some(parity) = parity_contents {
        std::fs::write(dir.join("parity_evidence.json"), parity).expect("write parity file");
    }
    (manifest, dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// TC-01: not_configured → start proceeds to DB gate (503)
// ---------------------------------------------------------------------------

/// TV-03C / TC-01: When MQK_ARTIFACT_PATH is not set, the parity gate is
/// NotConfigured (not applicable) and start proceeds to the DB gate (503).
///
/// This proves that absent artifact configuration does not produce a false
/// parity-not-configured error.
#[tokio::test]
async fn tc01_not_configured_passes_to_db_gate() {
    let _guard = env_lock().lock().await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "TC-01: not_configured must reach DB gate (503); body: {json}"
    );
}

// ---------------------------------------------------------------------------
// TC-02: absent parity evidence → 403 gate=parity_evidence
// ---------------------------------------------------------------------------

/// TV-03C / TC-02: When artifact intake succeeds and the deployability gate
/// passes, but `parity_evidence.json` is absent, start is refused at the
/// parity evidence gate with an explicit gate name.
///
/// Absent evidence ≠ parity proven.  Fail-closed.
#[tokio::test]
async fn tc02_absent_parity_blocks_start() {
    let _guard = env_lock().lock().await;
    let artifact_id = "tv03c-tc02-no-parity-artifact";
    // No parity_evidence.json written.
    let (manifest, dir) = write_artifact_dir("tc02", artifact_id, None);

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let json = parse_json(body);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "TC-02: absent parity evidence must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "TC-02: gate must be 'parity_evidence'; body: {json}"
    );
    let error_msg = json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("parity_evidence"),
        "TC-02: error message must mention parity_evidence; got: {error_msg}"
    );
}

// ---------------------------------------------------------------------------
// TC-03: invalid parity evidence → 403 gate=parity_evidence
// ---------------------------------------------------------------------------

/// TV-03C / TC-03: When artifact intake succeeds and the deployability gate
/// passes, but `parity_evidence.json` contains an invalid schema version,
/// start is refused at the parity evidence gate with an explicit gate name.
///
/// Invalid evidence ≠ parity proven.  Fail-closed.
#[tokio::test]
async fn tc03_invalid_parity_blocks_start() {
    let _guard = env_lock().lock().await;
    let artifact_id = "tv03c-tc03-bad-parity-schema";
    let bad_parity =
        r#"{"schema_version":"parity-v99","artifact_id":"x","live_trust_complete":false}"#;
    let (manifest, dir) = write_artifact_dir("tc03", artifact_id, Some(bad_parity));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let json = parse_json(body);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "TC-03: invalid parity evidence must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "TC-03: gate must be 'parity_evidence'; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// TC-04: valid parity evidence → proceeds to DB gate (503)
// ---------------------------------------------------------------------------

/// TV-03C / TC-04: When artifact intake succeeds, the deployability gate
/// passes, and `parity_evidence.json` is valid, start proceeds past the
/// parity gate to the DB gate (503 since no DB in test).
///
/// Present evidence is start-safe.
#[tokio::test]
async fn tc04_present_parity_passes_to_db_gate() {
    let _guard = env_lock().lock().await;
    let artifact_id = "tv03c-tc04-present-parity";
    let parity = valid_parity_json(artifact_id);
    let (manifest, dir) = write_artifact_dir("tc04", artifact_id, Some(&parity));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let json = parse_json(body);
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "TC-04: present parity evidence must reach DB gate (503); body: {json}"
    );
    // Must NOT be blocked by parity gate.
    let gate = json["gate"].as_str().unwrap_or("");
    assert_ne!(
        gate, "parity_evidence",
        "TC-04: must NOT be blocked by parity_evidence gate; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// TC-05: absent parity ordering — parity gate fires after artifact gate
// ---------------------------------------------------------------------------

/// TV-03C / TC-05: When MQK_ARTIFACT_PATH is set but points to a directory
/// without parity evidence, the parity gate fires (not the artifact intake
/// gate) — confirming gate ordering: TV-02C passes first, TV-03C blocks.
#[tokio::test]
async fn tc05_parity_gate_fires_after_artifact_gate() {
    let _guard = env_lock().lock().await;
    let artifact_id = "tv03c-tc05-gate-ordering";
    let (manifest, dir) = write_artifact_dir("tc05", artifact_id, None);

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let json = parse_json(body);
    // Must be blocked by parity_evidence, not by artifact_intake or
    // artifact_deployability.
    assert_eq!(status, StatusCode::FORBIDDEN, "TC-05: body: {json}");
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "TC-05: gate must be 'parity_evidence' not artifact gate; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// TC-06: mismatched parity artifact_id → 403 gate=parity_evidence
// ---------------------------------------------------------------------------

/// TV-03C / TC-06: When artifact intake succeeds, the deployability gate
/// passes, and `parity_evidence.json` is structurally valid, but the
/// `artifact_id` inside `parity_evidence.json` does not match the accepted
/// intake artifact_id, start is refused at the parity evidence gate.
///
/// This proves that the artifact-associated evidence chain cannot be satisfied
/// by parity evidence produced for a different artifact.  The TV-03C gate
/// cross-validates artifact identity — mirroring the TV-02C deployability gate
/// cross-validation — so a stale or misrouted `parity_evidence.json` cannot
/// stand in for the currently configured artifact's evidence.
#[tokio::test]
async fn tc06_mismatched_parity_artifact_id_blocks_start() {
    let _guard = env_lock().lock().await;
    let intake_artifact_id = "tv03c-tc06-real-artifact";
    let wrong_artifact_id = "tv03c-tc06-different-artifact";
    // Write parity evidence stamped with a DIFFERENT artifact_id than the manifest.
    let parity_for_wrong_artifact = valid_parity_json(wrong_artifact_id);
    let (manifest, dir) =
        write_artifact_dir("tc06", intake_artifact_id, Some(&parity_for_wrong_artifact));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    let st = armed_live_shadow_state().await;
    let (status, body) = call(routes::build_router(st), post_start()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let json = parse_json(body);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "TC-06: mismatched parity artifact_id must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "TC-06: gate must be 'parity_evidence' for artifact_id mismatch; body: {json}"
    );
    let error_msg = json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("does not match") || error_msg.contains("mismatch"),
        "TC-06: error message must describe the artifact_id mismatch; got: {error_msg}"
    );
}
