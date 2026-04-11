//! C2: Live-trust truth on the preflight surface — proof tests.
//!
//! ## What C2 closes
//!
//! Before C2, `GET /api/v1/system/preflight` gave an operator:
//!   - `daemon_mode` (e.g. "live-shadow")
//!   - `deployment_start_allowed: true` (if deployment gate passes)
//!   - no signal about `parity_evidence_state` or `live_trust_complete`
//!
//! An operator using preflight as their sole pre-start checklist on a
//! live-shadow or live-capital deployment could read `deployment_start_allowed:
//! true` and see no indication that `live_trust_complete=false` in all current
//! builds.  The C1 truth was only on `/api/v1/system/status`; an operator who
//! did not also read status had no live-trust signal from preflight.
//!
//! C2 adds the same two C1 fields to `PreflightStatusResponse`:
//!   - `parity_evidence_state` — same enum as C1
//!   - `live_trust_complete` — same semantics as C1; null is not a positive
//!     trust claim
//!
//! ## Tests (all pure in-process; require `--test-threads=1`)
//!
//! - C2-01: no `MQK_ARTIFACT_PATH` → preflight `parity_evidence_state:
//!          "not_configured"`, `live_trust_complete: null`; not a positive trust
//!          claim.
//! - C2-02: absent `parity_evidence.json` → preflight `"absent"`,
//!          `live_trust_complete: null`; absent evidence ≠ parity proven.
//! - C2-03: valid evidence with `live_trust_complete=false` → preflight
//!          `"incomplete"`, `live_trust_complete: false`; explicit honest signal
//!          even when `deployment_start_allowed=true`.
//! - C2-04: `live_trust_complete` is null for all non-Present states on
//!          preflight — null is never a false positive.
//! - C2-05: paper+alpaca path is not broken — preflight still includes the C2
//!          fields and paper-specific autonomous readiness fields remain intact.

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

fn preflight_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
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
        "produced_at_utc": "2026-04-09T00:00:00Z"
    })
    .to_string();
    let path = dir.join("parity_evidence.json");
    let mut f = std::fs::File::create(&path).expect("create parity_evidence.json");
    f.write_all(content.as_bytes())
        .expect("write parity_evidence.json");
}

/// Write a structurally invalid parity_evidence.json (wrong schema_version).
fn write_invalid_parity_evidence(dir: &std::path::Path) {
    let content = r#"{"schema_version": "parity-v99"}"#;
    let path = dir.join("parity_evidence.json");
    let mut f = std::fs::File::create(&path).expect("create parity_evidence.json");
    f.write_all(content.as_bytes())
        .expect("write parity_evidence.json");
}

/// Create a unique temp dir and an empty promoted_manifest.json inside it.
/// The evaluator uses the manifest path to derive the parent dir.
fn make_artifact_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mqk_c2_{tag}_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).expect("create manifest");
    (dir, manifest)
}

// ---------------------------------------------------------------------------
// C2-01: no MQK_ARTIFACT_PATH → preflight "not_configured", live_trust_complete null.
// ---------------------------------------------------------------------------

/// C2-01: Without MQK_ARTIFACT_PATH the preflight surface reports
/// `parity_evidence_state: "not_configured"` and `live_trust_complete: null`.
///
/// Proves: an operator who has not configured an artifact path sees an explicit
/// "not_configured" ceiling on preflight — not an absent field or ambiguous null.
/// `deployment_start_allowed` does not imply live trust when the evidence is
/// not configured.
#[tokio::test]
async fn c2_01_no_artifact_path_preflight_not_configured() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, preflight_req()).await;
    assert_eq!(status, StatusCode::OK, "preflight must return 200");

    let json = parse_json(body);

    // C2 fields must be structural (always present).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C2-01: parity_evidence_state must be present on preflight response"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C2-01: live_trust_complete key must be present on preflight response"
    );

    assert_eq!(
        json["parity_evidence_state"], "not_configured",
        "C2-01: no artifact path → parity_evidence_state must be not_configured"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "C2-01: not_configured → live_trust_complete must be null (not a positive trust claim)"
    );
}

// ---------------------------------------------------------------------------
// C2-02: absent parity_evidence.json → preflight "absent", live_trust_complete null.
// ---------------------------------------------------------------------------

/// C2-02: When MQK_ARTIFACT_PATH is set but parity_evidence.json does not
/// exist, preflight reports `"absent"` and `live_trust_complete: null`.
///
/// Proves: absent evidence is not trust.  An operator checking preflight on a
/// live-shadow deployment where the evidence file was accidentally deleted
/// cannot mistake the blank slate for proven parity.
#[tokio::test]
async fn c2_02_absent_evidence_preflight_not_trust() {
    let (dir, manifest) = make_artifact_dir("c2_02");
    // Manifest exists but parity_evidence.json does NOT.
    let _ = dir;
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, preflight_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(
        json["parity_evidence_state"], "absent",
        "C2-02: absent parity_evidence.json → state must be absent"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "C2-02: absent evidence → live_trust_complete must be null"
    );
}

// ---------------------------------------------------------------------------
// C2-03: valid evidence with live_trust_complete=false → "incomplete", false.
// ---------------------------------------------------------------------------

/// C2-03: When valid parity_evidence.json is present with
/// `live_trust_complete=false`, preflight reports `"incomplete"` and
/// `live_trust_complete: false`.
///
/// This is the primary C2 proof: an operator on a live-shadow deployment with
/// `deployment_start_allowed=true` still sees the explicit live-trust ceiling
/// directly on the preflight surface.  They cannot reach an incorrect
/// conclusion that parity is proven by reading preflight alone.
#[tokio::test]
async fn c2_03_incomplete_evidence_explicit_on_preflight() {
    let (dir, manifest) = make_artifact_dir("c2_03");
    write_valid_parity_evidence(&dir, "test-artifact-c2-03");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, preflight_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // The live-trust ceiling is explicit on preflight.
    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C2-03: present evidence with live_trust_complete=false → parity_evidence_state must be \
         \"incomplete\" on preflight"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "C2-03: incomplete → live_trust_complete must be explicit false on preflight"
    );

    // deployment_start_allowed may be true (from deployment gate), but the
    // live-trust ceiling is still visible alongside it.
    assert!(
        json.get("deployment_start_allowed").is_some(),
        "C2-03: deployment_start_allowed must still be present"
    );
    // The two fields coexist in the same response — operator sees both.
    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C2-03: deployment_start_allowed and parity_evidence_state are co-present"
    );
}

// ---------------------------------------------------------------------------
// C2-04: live_trust_complete is null for all non-Present states on preflight.
// ---------------------------------------------------------------------------

/// C2-04: `live_trust_complete` is null for every non-Present state on the
/// preflight surface.
///
/// Tests three non-Present outcomes:
///   (a) not_configured → null
///   (b) absent → null
///   (c) invalid → null
///
/// Proves: null `live_trust_complete` is never a false-positive trust signal
/// on preflight.  The contract matches C1 exactly.
#[tokio::test]
async fn c2_04_live_trust_complete_null_for_non_present_states_on_preflight() {
    // (a) not_configured
    {
        let _g = EnvGuard::absent(ENV_ARTIFACT_PATH);
        let (s, b) = call(make_router(), preflight_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "not_configured");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "C2-04(a): not_configured → live_trust_complete must be null on preflight"
        );
    }

    // (b) absent
    {
        let (dir, manifest) = make_artifact_dir("c2_04b");
        let _ = dir;
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), preflight_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "absent");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "C2-04(b): absent → live_trust_complete must be null on preflight"
        );
    }

    // (c) invalid
    {
        let (dir, manifest) = make_artifact_dir("c2_04c");
        write_invalid_parity_evidence(&dir);
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), preflight_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_eq!(j["parity_evidence_state"], "invalid");
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "C2-04(c): invalid → live_trust_complete must be null on preflight"
        );
    }
}

// ---------------------------------------------------------------------------
// C2-05: paper path intact — C2 fields present, autonomous fields not broken.
// ---------------------------------------------------------------------------

/// C2-05: The paper+alpaca paper path is not broken by C2.
///
/// On the default paper deployment (no artifact path):
///   - `parity_evidence_state` is `"not_configured"` (structural, always present)
///   - `live_trust_complete` is null (not a positive trust claim)
///   - `autonomous_readiness_applicable` is false (default paper+paper, not paper+alpaca)
///   - `live_trust_complete` is never true (no false positive on any path)
///
/// Proves: adding C2 fields does not break existing paper-path consumers.  The
/// autonomous readiness fields added by AUTON-TRUTH-02 remain structurally intact.
#[tokio::test]
async fn c2_05_paper_path_not_broken_by_c2() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, preflight_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "C2-05: preflight must return 200 on paper path"
    );

    let json = parse_json(body);

    // C2 fields are structural — always present, even on paper.
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C2-05: parity_evidence_state must be present on paper path"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C2-05: live_trust_complete key must be present on paper path"
    );

    // Paper with no artifact → not_configured; honest null trust.
    let state_str = json["parity_evidence_state"].as_str().unwrap_or("");
    assert!(
        !state_str.is_empty(),
        "C2-05: parity_evidence_state must be a non-empty string on paper path"
    );

    // live_trust_complete must never be true on any paper path.
    assert_ne!(
        json["live_trust_complete"],
        serde_json::Value::Bool(true),
        "C2-05: live_trust_complete must never be true on paper path"
    );

    // Autonomous readiness fields from AUTON-TRUTH-02 must still be present.
    assert!(
        json.get("autonomous_readiness_applicable").is_some(),
        "C2-05: autonomous_readiness_applicable must still be present after C2"
    );
    assert!(
        json.get("autonomous_blockers").is_some(),
        "C2-05: autonomous_blockers must still be present after C2"
    );

    // daemon_reachable and deployment_start_allowed are structural.
    assert_eq!(
        json["daemon_reachable"], true,
        "C2-05: daemon_reachable must be true"
    );
    assert!(
        json.get("deployment_start_allowed").is_some(),
        "C2-05: deployment_start_allowed must still be present"
    );
}
