//! TV-02A / TV-02B / TV-02C: Artifact deployability gate proof.
//!
//! Proves that the runtime start boundary distinguishes between:
//!   - artifact accepted for intake (structural acceptance only)
//!   - artifact deployable/tradable (passes TV-02 deployability gate)
//!   - artifact refused at start (non-deployable, gate absent, gate invalid)
//!
//! TV-02A: Minimum deployability/tradability gate seam exists and is wired into
//!         `start_execution_runtime` before DB operations.
//! TV-02B: Gate criteria (`passed` field from TV-02 Python pipeline) are enforced
//!         at the start boundary — the Rust gate trusts the Python gate result.
//! TV-02C: Non-deployable artifacts are refused at `POST /v1/run/start` with
//!         explicit, fail-closed semantics before any DB operations.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                               |
//! |------|------------------------------------------------------------------------------|
//! | D01  | No artifact configured → gate not applicable → start proceeds to DB gate    |
//! | D02  | Accepted + gate passed=true → start proceeds to DB gate (gate transparent)  |
//! | D03  | Accepted + gate absent → start blocked: gate=artifact_deployability         |
//! | D04  | Accepted + gate passed=false → start blocked: gate=artifact_deployability    |
//! | D05  | Accepted + gate invalid schema → start blocked: gate=artifact_deployability  |
//! | D06  | Accepted + gate artifact_id mismatch → start blocked: gate=artifact_deployability|
//! | D07  | Intake invalid (configured but broken file) → start blocked: gate=artifact_intake |
//! | D08  | Pure evaluator: NotConfigured path → NotConfigured outcome                   |
//! | D09  | Pure evaluator: gate absent → GateAbsent                                    |
//! | D10  | Pure evaluator: gate passed=false → NotDeployable with reason               |
//! | D11  | Pure evaluator: gate passed=true + id match → Deployable                    |
//! | D12  | Pure evaluator: artifact_id mismatch → GateInvalid                          |
//!
//! D01..D07 go through `POST /v1/run/start` (same as all other start-gate tests).
//! D08..D12 are pure in-process unit tests of `evaluate_artifact_deployability`.
//! All tests require no database and no network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    artifact_intake::{evaluate_artifact_deployability, ArtifactDeployabilityOutcome},
    routes, state,
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialisation — same pattern as scenario_artifact_intake_tv01b.rs
// ---------------------------------------------------------------------------

/// Serialises tests that mutate `MQK_ARTIFACT_PATH` so they do not race.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// Unique counter for temp dir names.
static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// HTTP helpers — same pattern as scenario_ws_continuity_gate_brk00r04.rs
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

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Build a minimal valid `promoted_manifest.json` content.
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

/// Build a `deployability_gate.json` content for `artifact_id` with given `passed`.
fn gate_json(artifact_id: &str, passed: bool) -> String {
    let (passed_str, overall) = if passed {
        (
            "true",
            "All four deployability checks passed: trade count, sample window, \
             daily turnover, and active day fraction are within bounds.",
        )
    } else {
        (
            "false",
            "Gate FAILED. Failed checks: min_trade_count. This artifact does not \
             meet minimum tradability or sample adequacy criteria.",
        )
    };
    format!(
        r#"{{
  "schema_version": "gate-v1",
  "artifact_id": "{artifact_id}",
  "passed": {passed_str},
  "checks": [],
  "overall_reason": "{overall}",
  "evaluated_at_utc": "2026-01-01T00:00:00Z"
}}"#
    )
}

/// Write a temp artifact dir with a `promoted_manifest.json` and (optionally) a
/// `deployability_gate.json`.  Returns `(manifest_path, artifact_dir)`.
fn write_artifact_dir(tag: &str, artifact_id: &str, gate: Option<&str>) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_tv02_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create artifact dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(artifact_id)).expect("write manifest");
    if let Some(gate_contents) = gate {
        std::fs::write(dir.join("deployability_gate.json"), gate_contents)
            .expect("write gate file");
    }
    (manifest, dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Build a minimal valid `parity_evidence.json` content (TV-03C gate requires
/// this when an artifact path is configured).
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
        "live_trust_gaps": [],
        "produced_at_utc": "2026-03-01T00:00:00Z"
    })
    .to_string()
}

/// Build an armed LiveShadow+Alpaca AppState.
///
/// LiveShadow+Alpaca:
///   - start_allowed = true (deployment gate passes)
///   - no paper+alpaca WS continuity gate (only fires for Paper+Alpaca)
///   - no live-capital operator token gate (only fires for LiveCapital)
///   - no live-capital WS continuity gate (only fires for LiveCapital)
///
/// After arming integrity, the gate sequence is:
///   1. deployment_readiness → passes
///   2. integrity → passes (armed)
///   3. [paper+alpaca WS gates] → not applicable
///   4. [live-capital WS gate] → not applicable
///   5. TV-02C artifact deployability gate ← what these tests exercise
///   6. db_pool() → ServiceUnavailable (no DB in test)
async fn armed_live_shadow_state() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    // Arm integrity via the same HTTP route all other tests use.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "test setup: arm must succeed");
    st
}

async fn post_start(st: Arc<state::AppState>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(st), req).await;
    let json = parse_json(body);
    (status, json)
}

// ---------------------------------------------------------------------------
// D01: No artifact configured → deployability gate not applicable → DB gate
// ---------------------------------------------------------------------------

/// TV-02C / D01: When `MQK_ARTIFACT_PATH` is not configured, the deployability
/// gate is not applicable and start proceeds to the DB gate (503).
///
/// Proves that absence of an artifact does NOT block start at the
/// deployability gate.
#[tokio::test]
async fn d01_no_artifact_configured_passes_to_db_gate() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("MQK_ARTIFACT_PATH");

    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;

    // Must reach DB gate (503), not artifact_deployability (403).
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "D01: no artifact → must reach DB gate (503); gate was: {:?}; body: {json}",
        json["gate"]
    );
    assert_ne!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D01: deployability gate must not fire when no artifact configured; body: {json}"
    );
    assert_ne!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_intake",
        "D01: intake gate must not fire when no artifact configured; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// D02: Accepted + gate passed=true → deployability gate passes → DB gate
// ---------------------------------------------------------------------------

/// TV-02C / D02: When artifact intake succeeds AND deployability gate has
/// `passed=true`, start is NOT blocked by the deployability gate and proceeds
/// to the DB gate.
///
/// This is the happy-path proof: accepted + deployable → gate transparent.
#[tokio::test]
async fn d02_accepted_deployable_passes_to_db_gate() {
    let _guard = env_lock().lock().unwrap();
    let artifact_id = "tv02-d02-deployable-artifact-abc123";
    let gate_contents = gate_json(artifact_id, true);
    let (manifest, dir) = write_artifact_dir("d02", artifact_id, Some(&gate_contents));
    // TV-03C gate: parity evidence required when artifact path is configured.
    std::fs::write(
        dir.join("parity_evidence.json"),
        valid_parity_json(artifact_id),
    )
    .expect("write parity file");

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "D02: accepted+deployable → must reach DB gate (503); gate was: {:?}; body: {json}",
        json["gate"]
    );
    assert_ne!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D02: deployability gate must not fire when artifact is deployable; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// D03: Accepted + gate absent → start blocked at artifact_deployability
// ---------------------------------------------------------------------------

/// TV-02C / D03: When artifact intake succeeds but no `deployability_gate.json`
/// exists in the artifact directory, start is refused at the deployability gate.
///
/// Absent gate ≠ deployable. Fail-closed.
#[tokio::test]
async fn d03_accepted_gate_absent_blocks_start() {
    let _guard = env_lock().lock().unwrap();
    let artifact_id = "tv02-d03-no-gate-artifact-xyz";
    let (manifest, dir) = write_artifact_dir("d03", artifact_id, None);

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D03: absent gate → must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D03: must block at artifact_deployability gate; body: {json}"
    );
    assert_eq!(
        json["fault_class"].as_str().unwrap_or(""),
        "runtime.start_refused.artifact_not_deployable",
        "D03: fault_class must identify deployability refusal; body: {json}"
    );
    let error = json["error"].as_str().unwrap_or("");
    assert!(
        error.contains("gate_absent"),
        "D03: error must cite 'gate_absent'; got: {error}"
    );
}

// ---------------------------------------------------------------------------
// D04: Accepted + gate passed=false → start blocked at artifact_deployability
// ---------------------------------------------------------------------------

/// TV-02C / TV-02B / D04: When artifact intake succeeds but the deployability
/// gate has `passed=false`, start is refused.
///
/// A structurally accepted artifact that did not clear the TV-02 minimum
/// criteria (min_trade_count etc.) cannot start runtime.
#[tokio::test]
async fn d04_accepted_gate_failed_blocks_start() {
    let _guard = env_lock().lock().unwrap();
    let artifact_id = "tv02-d04-failing-gate-artifact-qrs";
    let gate_contents = gate_json(artifact_id, false);
    let (manifest, dir) = write_artifact_dir("d04", artifact_id, Some(&gate_contents));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D04: failed gate → must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D04: must block at artifact_deployability gate; body: {json}"
    );
    assert_eq!(
        json["fault_class"].as_str().unwrap_or(""),
        "runtime.start_refused.artifact_not_deployable",
        "D04: fault_class must identify deployability refusal; body: {json}"
    );
    let error = json["error"].as_str().unwrap_or("");
    assert!(
        error.contains("not_deployable"),
        "D04: error must cite 'not_deployable'; got: {error}"
    );
}

// ---------------------------------------------------------------------------
// D05: Accepted + gate invalid schema → start blocked at artifact_deployability
// ---------------------------------------------------------------------------

/// TV-02C / D05: When the gate file has an unrecognised `schema_version`,
/// start is refused at the deployability gate.  Gate validity is enforced.
#[tokio::test]
async fn d05_accepted_gate_invalid_schema_blocks_start() {
    let _guard = env_lock().lock().unwrap();
    let artifact_id = "tv02-d05-bad-schema-artifact-uvw";
    let bad_gate = format!(
        r#"{{"schema_version":"gate-v99","artifact_id":"{artifact_id}","passed":true,"checks":[],"overall_reason":"","evaluated_at_utc":"2026-01-01T00:00:00Z"}}"#
    );
    let (manifest, dir) = write_artifact_dir("d05", artifact_id, Some(&bad_gate));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D05: invalid gate schema → must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D05: must block at artifact_deployability; body: {json}"
    );
    let error = json["error"].as_str().unwrap_or("");
    assert!(
        error.contains("gate_invalid"),
        "D05: error must cite 'gate_invalid'; got: {error}"
    );
}

// ---------------------------------------------------------------------------
// D06: Accepted + gate artifact_id mismatch → start blocked
// ---------------------------------------------------------------------------

/// TV-02C / D06: When the gate file's `artifact_id` differs from the intake
/// artifact_id, start is refused.  Cross-validation is enforced.
#[tokio::test]
async fn d06_gate_artifact_id_mismatch_blocks_start() {
    let _guard = env_lock().lock().unwrap();
    let intake_artifact_id = "tv02-d06-intake-artifact-id";
    let gate_artifact_id = "tv02-d06-DIFFERENT-artifact-id";

    // Gate says passed=true but for a different artifact.
    let gate_contents = gate_json(gate_artifact_id, true);
    let (manifest, dir) = write_artifact_dir("d06", intake_artifact_id, Some(&gate_contents));

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D06: artifact_id mismatch → must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "D06: must block at artifact_deployability; body: {json}"
    );
    let error = json["error"].as_str().unwrap_or("");
    assert!(
        error.contains("gate_invalid"),
        "D06: error must cite 'gate_invalid' for id mismatch; got: {error}"
    );
}

// ---------------------------------------------------------------------------
// D07: Intake invalid (configured path → non-existent file) → intake gate
// ---------------------------------------------------------------------------

/// TV-02C / D07: When `MQK_ARTIFACT_PATH` is configured but points to a
/// non-existent file, start is refused at the `artifact_intake` gate (before
/// the deployability gate is evaluated).
///
/// Configured-but-broken artifacts are fail-closed at the intake boundary.
#[tokio::test]
async fn d07_intake_invalid_blocks_start_at_intake_gate() {
    let _guard = env_lock().lock().unwrap();
    let path = std::env::temp_dir().join(format!(
        "mqk_tv02_d07_does_not_exist_{}_{}.json",
        std::process::id(),
        next_id()
    ));
    let _ = std::fs::remove_file(&path);

    std::env::set_var("MQK_ARTIFACT_PATH", path.to_str().unwrap());
    let st = armed_live_shadow_state().await;
    let (status, json) = post_start(st).await;
    std::env::remove_var("MQK_ARTIFACT_PATH");

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D07: invalid intake → must return 403; body: {json}"
    );
    assert_eq!(
        json["gate"].as_str().unwrap_or(""),
        "artifact_intake",
        "D07: must block at artifact_intake gate; body: {json}"
    );
    assert_eq!(
        json["fault_class"].as_str().unwrap_or(""),
        "runtime.start_refused.artifact_intake_invalid",
        "D07: fault_class must identify intake invalidity; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// D08..D12: Pure evaluator unit tests (no env var, no AppState, no HTTP)
// ---------------------------------------------------------------------------

/// TV-02A / D08: `evaluate_artifact_deployability(None, _)` → `NotConfigured`.
#[test]
fn d08_pure_none_path_returns_not_configured() {
    let out = evaluate_artifact_deployability(None, "any-id");
    assert_eq!(
        out,
        ArtifactDeployabilityOutcome::NotConfigured,
        "D08: None path must return NotConfigured"
    );
    assert!(
        !out.is_deployable(),
        "D08: NotConfigured must not be deployable"
    );
    assert_eq!(out.truth_state(), "not_configured");
}

/// TV-02A / D09: Valid manifest path but no gate file → `GateAbsent`.
#[test]
fn d09_pure_gate_absent_returns_gate_absent() {
    let id = next_id();
    let dir = std::env::temp_dir().join(format!("mqk_tv02_d09_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest("d09-id")).expect("write manifest");
    // No deployability_gate.json.

    let out = evaluate_artifact_deployability(Some(&manifest), "d09-id");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        out,
        ArtifactDeployabilityOutcome::GateAbsent,
        "D09: absent gate file must return GateAbsent; got: {out:?}"
    );
    assert!(
        !out.is_deployable(),
        "D09: GateAbsent must not be deployable"
    );
    assert_eq!(out.truth_state(), "gate_absent");
}

/// TV-02B / D10: Gate file with `passed=false` → `NotDeployable` with reason.
#[test]
fn d10_pure_gate_not_passed_returns_not_deployable() {
    let id = next_id();
    let artifact_id = format!("d10-artifact-{id}");
    let dir = std::env::temp_dir().join(format!("mqk_tv02_d10_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(&artifact_id)).expect("write manifest");
    std::fs::write(
        dir.join("deployability_gate.json"),
        gate_json(&artifact_id, false),
    )
    .expect("write gate");

    let out = evaluate_artifact_deployability(Some(&manifest), &artifact_id);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(out, ArtifactDeployabilityOutcome::NotDeployable { .. }),
        "D10: passed=false must return NotDeployable; got: {out:?}"
    );
    assert!(
        !out.is_deployable(),
        "D10: NotDeployable must not be deployable"
    );
    assert_eq!(out.truth_state(), "not_deployable");
    if let ArtifactDeployabilityOutcome::NotDeployable { overall_reason } = &out {
        assert!(
            !overall_reason.is_empty(),
            "D10: overall_reason must be non-empty"
        );
        assert!(
            overall_reason.contains("FAILED"),
            "D10: reason must mention FAILED; got: {overall_reason}"
        );
    }
}

/// TV-02B / D11: Gate file with `passed=true` and matching `artifact_id` →
/// `Deployable` with the same artifact_id.
#[test]
fn d11_pure_gate_passed_true_returns_deployable() {
    let id = next_id();
    let artifact_id = format!("d11-artifact-{id}");
    let dir = std::env::temp_dir().join(format!("mqk_tv02_d11_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(&artifact_id)).expect("write manifest");
    std::fs::write(
        dir.join("deployability_gate.json"),
        gate_json(&artifact_id, true),
    )
    .expect("write gate");

    let out = evaluate_artifact_deployability(Some(&manifest), &artifact_id);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(
            &out,
            ArtifactDeployabilityOutcome::Deployable { artifact_id: id }
                if id == &artifact_id
        ),
        "D11: passed=true must return Deployable with matching artifact_id; got: {out:?}"
    );
    assert!(
        out.is_deployable(),
        "D11: Deployable must be is_deployable()"
    );
    assert_eq!(out.truth_state(), "deployable");
}

/// TV-02A / D12: Gate file `artifact_id` mismatch → `GateInvalid`.
#[test]
fn d12_pure_artifact_id_mismatch_returns_gate_invalid() {
    let id = next_id();
    let intake_id = format!("d12-intake-{id}");
    let gate_id = format!("d12-gate-different-{id}");
    let dir = std::env::temp_dir().join(format!("mqk_tv02_d12_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(&intake_id)).expect("write manifest");
    // Gate says passed=true but carries a different artifact_id.
    std::fs::write(
        dir.join("deployability_gate.json"),
        gate_json(&gate_id, true),
    )
    .expect("write gate");

    let out = evaluate_artifact_deployability(Some(&manifest), &intake_id);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(out, ArtifactDeployabilityOutcome::GateInvalid { .. }),
        "D12: artifact_id mismatch must return GateInvalid; got: {out:?}"
    );
    assert!(
        !out.is_deployable(),
        "D12: GateInvalid must not be deployable"
    );
    assert_eq!(out.truth_state(), "gate_invalid");
    if let ArtifactDeployabilityOutcome::GateInvalid { reason } = &out {
        assert!(
            reason.contains("does not match"),
            "D12: reason must mention id mismatch; got: {reason}"
        );
    }
}
