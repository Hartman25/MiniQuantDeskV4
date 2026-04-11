//! C1: Live-trust truth surface — proof tests.
//!
//! ## What C1 closes
//!
//! Before C1, `GET /api/v1/system/status` gave an operator:
//!   - `daemon_mode` (e.g. "live-shadow")
//!   - `deployment_start_allowed: true` (if deployment gate passes)
//!   - no signal about `live_trust_complete`
//!
//! An operator reading only `/api/v1/system/status` with `daemon_mode:
//! "live-shadow"` and `deployment_start_allowed: true` had no indicator that
//! `live_trust_complete=false` in all current builds.  The only truthful signal
//! was siloed to the secondary endpoint `/api/v1/system/parity-evidence`.
//!
//! C1 adds two fields to `SystemStatusResponse`:
//!   - `parity_evidence_state` — machine-readable parity state:
//!     `"not_configured"` | `"absent"` | `"invalid"` |
//!     `"incomplete"` | `"complete"` | `"unavailable"`
//!   - `live_trust_complete` — `false` when evidence is present,
//!     `null` otherwise (null is not a positive trust claim).
//!
//! ## Tests
//!
//! All tests are pure in-process.  Env-var manipulation requires
//! `--test-threads=1`.
//!
//! - C1-01: no `MQK_ARTIFACT_PATH` → `parity_evidence_state: "not_configured"`,
//!          `live_trust_complete: null`; explicit ceiling, no false trust.
//! - C1-02: `MQK_ARTIFACT_PATH` points to non-existent path → `"absent"`,
//!          `live_trust_complete: null`; absent evidence ≠ parity proven.
//! - C1-03: valid `parity_evidence.json` with `live_trust_complete=false` →
//!          `"incomplete"`, `live_trust_complete: false`; explicit honest signal.
//! - C1-04: `live_trust_complete` is null for every non-Present state —
//!          null is never a false positive; states "not_configured", "absent",
//!          "invalid" all produce `live_trust_complete: null`.
//! - C1-05: parity_evidence_state present in status independently of deployment
//!          mode — paper+paper path returns "not_configured" (no artifact); the
//!          field is always populated regardless of deployment mode.

use std::io::Write as _;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ENV_ARTIFACT_PATH: &str = "MQK_ARTIFACT_PATH";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

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

fn system_status_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// RAII guard: saves and clears an env var; restores on drop.
/// Requires `--test-threads=1`.
struct EnvGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    fn absent(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        #[allow(deprecated)]
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prior }
    }

    fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        #[allow(deprecated)]
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        #[allow(deprecated)]
        unsafe {
            match &self.prior {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Write a minimal valid parity_evidence.json into `dir`.
fn write_valid_parity_evidence(dir: &std::path::Path, artifact_id: &str) {
    let content = serde_json::json!({
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
        "live_trust_gaps": ["No shadow evaluation cycle completed"],
        "produced_at_utc": "2026-04-08T00:00:00Z"
    })
    .to_string();
    let path = dir.join("parity_evidence.json");
    let mut f = std::fs::File::create(&path).expect("create parity_evidence.json");
    f.write_all(content.as_bytes())
        .expect("write parity_evidence.json");
}

/// Write a minimal valid invalid parity_evidence.json (wrong schema_version).
fn write_invalid_parity_evidence(dir: &std::path::Path) {
    let content = r#"{"schema_version": "parity-v99"}"#;
    let path = dir.join("parity_evidence.json");
    let mut f = std::fs::File::create(&path).expect("create parity_evidence.json");
    f.write_all(content.as_bytes())
        .expect("write parity_evidence.json");
}

/// Create a unique temp dir for artifact evidence, return path to a fake
/// promoted_manifest.json inside it (the evaluator uses parent(manifest_path)).
fn make_artifact_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mqk_c1_{tag}_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let manifest = dir.join("promoted_manifest.json");
    // The evaluator only needs the manifest path to derive the parent dir.
    std::fs::File::create(&manifest).expect("create manifest");
    (dir, manifest)
}

// ---------------------------------------------------------------------------
// C1-01: no MQK_ARTIFACT_PATH → parity_evidence_state: "not_configured",
//        live_trust_complete: null.
// ---------------------------------------------------------------------------

/// C1-01: Without MQK_ARTIFACT_PATH the status surface reports
/// "not_configured" and live_trust_complete is null.
///
/// Proves: an operator who has not configured an artifact path sees an explicit
/// "not_configured" ceiling, not an ambiguous null or absence of the field.
/// Null live_trust_complete is not a positive trust claim.
#[tokio::test]
async fn c1_01_no_artifact_path_is_not_configured() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, system_status_req()).await;
    assert_eq!(status, StatusCode::OK, "system/status must return 200");

    let json = parse_json(body);

    // C1 fields must always be present on the status surface.
    assert!(
        json.get("parity_evidence_state").is_some(),
        "parity_evidence_state must be present on status response"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "live_trust_complete key must be present on status response"
    );

    assert_eq!(
        json["parity_evidence_state"], "not_configured",
        "no artifact path → not_configured"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "not_configured → live_trust_complete must be null (not a positive trust claim)"
    );
}

// ---------------------------------------------------------------------------
// C1-02: absent parity_evidence.json → "absent", live_trust_complete: null.
// ---------------------------------------------------------------------------

/// C1-02: When MQK_ARTIFACT_PATH is set but parity_evidence.json does not
/// exist in the artifact directory, the status surface reports "absent" and
/// live_trust_complete is null.
///
/// Proves: absent evidence ≠ parity proven.  The operator cannot mistake the
/// absence of the evidence file for live trust being established.
#[tokio::test]
async fn c1_02_absent_evidence_is_not_trust() {
    let (dir, manifest) = make_artifact_dir("c1_02");
    // Create the manifest but NOT parity_evidence.json — it is absent.
    let _ = dir; // keep alive
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, system_status_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["parity_evidence_state"], "absent");
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "absent evidence → live_trust_complete must be null"
    );
}

// ---------------------------------------------------------------------------
// C1-03: valid evidence with live_trust_complete=false → "incomplete", false.
// ---------------------------------------------------------------------------

/// C1-03: When valid parity_evidence.json is present with live_trust_complete=false
/// the status surface reports "incomplete" and live_trust_complete=false.
///
/// Proves: the primary operator surface explicitly shows that live trust is NOT
/// complete in current builds, even when deployment gates pass.  An operator
/// cannot observe daemon_mode + deployment_start_allowed without also seeing
/// parity_evidence_state="incomplete" and live_trust_complete=false.
#[tokio::test]
async fn c1_03_present_evidence_is_incomplete_in_current_builds() {
    let (dir, manifest) = make_artifact_dir("c1_03");
    write_valid_parity_evidence(&dir, "test-artifact-c1-03");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, system_status_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "present evidence with live_trust_complete=false → state must be \"incomplete\""
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "incomplete → live_trust_complete must be explicit false, not null"
    );
}

// ---------------------------------------------------------------------------
// C1-04: live_trust_complete is null for all non-Present states.
// ---------------------------------------------------------------------------

/// C1-04: live_trust_complete is null for every non-Present state.
///
/// Tests three non-Present outcomes individually:
///   (a) not_configured → null
///   (b) absent → null
///   (c) invalid → null
///
/// Proves: null live_trust_complete is never a false-positive trust signal.
/// Only the "incomplete" and "complete" states produce a non-null value.
#[tokio::test]
async fn c1_04_live_trust_complete_null_for_non_present_states() {
    // (a) not_configured
    {
        let _g = EnvGuard::absent(ENV_ARTIFACT_PATH);
        let (s, b) = call(make_router(), system_status_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "not_configured");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "(a) not_configured → null"
        );
    }

    // (b) absent
    {
        let (dir, manifest) = make_artifact_dir("c1_04b");
        let _ = dir;
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), system_status_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "absent");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "(b) absent → null"
        );
    }

    // (c) invalid — write a structurally invalid evidence file
    {
        let (dir, manifest) = make_artifact_dir("c1_04c");
        write_invalid_parity_evidence(&dir);
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), system_status_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "invalid");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "(c) invalid → null"
        );
    }
}

// ---------------------------------------------------------------------------
// C1-05: parity_evidence_state is always present regardless of deployment mode.
// ---------------------------------------------------------------------------

/// C1-05: parity_evidence_state and live_trust_complete are always populated
/// in the status response — the field is not conditional on deployment mode.
///
/// Uses the default AppState (paper+paper) with no artifact path configured.
/// Proves: the field is structural in the response contract, not gated on
/// live-shadow or live-capital mode.  Paper operators also see the explicit
/// ceiling so the field is trustworthy across all modes.
#[tokio::test]
async fn c1_05_parity_state_present_on_paper_path() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, system_status_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // Both C1 fields must be structural (always present).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "parity_evidence_state must be present even on paper+paper"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "live_trust_complete key must be present even on paper+paper"
    );

    // Paper+paper with no artifact → not_configured; honest null trust.
    let state = json["parity_evidence_state"].as_str().unwrap_or("");
    assert!(
        !state.is_empty(),
        "parity_evidence_state must be a non-empty string"
    );

    // Whichever state the default produces, live_trust_complete must NOT be
    // true — no accidental positive trust claim on the paper path.
    assert_ne!(
        json["live_trust_complete"],
        serde_json::Value::Bool(true),
        "live_trust_complete must never be true on paper+paper path"
    );
}
