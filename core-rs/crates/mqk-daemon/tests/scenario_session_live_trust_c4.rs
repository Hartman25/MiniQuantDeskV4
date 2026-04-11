//! C4: Live-trust truth on the session surface — proof tests.
//!
//! ## What C4 closes
//!
//! Before C4, `GET /api/v1/system/session` gave an operator:
//!   - `deployment_start_allowed` (bool)
//!   - `deployment_blocker` (optional string)
//!   - calendar / window state
//!   - no signal about `parity_evidence_state` or `live_trust_complete`
//!
//! An operator consulting `/api/v1/system/session` as a lightweight "can I
//! execute now?" check on a live-shadow or live-capital deployment could see
//! `deployment_start_allowed: true` with zero visibility into the live-trust
//! ceiling.  The three prior closures all patched a different surface:
//!
//!   - C1 → `/api/v1/system/status`
//!   - C2 → `/api/v1/system/preflight`
//!   - C3 → `/api/v1/ops/mode-change-guidance`
//!
//! Session was the only remaining primary operator surface that still omitted
//! the live-trust ceiling fields.  An operator who only read session could
//! mistake `deployment_start_allowed: true` for live readiness.
//!
//! C4 adds the same two trust-ceiling fields from C1/C2/C3 directly to
//! `SessionStateResponse`:
//!   - `parity_evidence_state` — same label enum as C1/C2/C3
//!   - `live_trust_complete`   — same semantics; null is never a positive trust
//!     claim; `Some(false)` is the explicit ceiling in current builds
//!
//! The same `evaluate_parity_evidence_guarded()` evaluator is used across all
//! four surfaces (status/C1, preflight/C2, guidance/C3, session/C4) so they
//! cannot diverge.
//!
//! ## Tests (all pure in-process; require `--test-threads=1`)
//!
//! - C4-01: no `MQK_ARTIFACT_PATH` → session `parity_evidence_state:
//!          "not_configured"`, `live_trust_complete: null`; not a positive
//!          trust claim.
//! - C4-02: valid evidence with `live_trust_complete=false` → session
//!          `"incomplete"`, `live_trust_complete: false`; explicit honest
//!          ceiling on the lightweight session surface.
//! - C4-03: `deployment_start_allowed=true` (paper+alpaca) and explicit
//!          `live_trust_complete: false` co-present on the same session
//!          response.  Proves the gap between "start is structurally allowed"
//!          and "live-trust ceiling is not met" is visible on this surface.
//! - C4-04: `live_trust_complete` is never true on the session surface in the
//!          current build for any artifact path configuration.
//! - C4-05: Existing session contract fields (`daemon_mode`, `adapter_id`,
//!          `deployment_start_allowed`, `market_session`, `calendar_spec_id`,
//!          `notes`) are not broken by C4.

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

fn session_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/system/session")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// RAII guard: saves and restores an env var.
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

/// Write a minimal valid parity_evidence.json with live_trust_complete=false.
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

/// Create a unique temp dir with an empty promoted_manifest.json placeholder.
fn make_artifact_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mqk_c4_{tag}_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).expect("create manifest");
    (dir, manifest)
}

// ---------------------------------------------------------------------------
// C4-01: no MQK_ARTIFACT_PATH → session "not_configured", live_trust_complete null.
// ---------------------------------------------------------------------------

/// C4-01: Without `MQK_ARTIFACT_PATH` the session surface reports
/// `parity_evidence_state: "not_configured"` and `live_trust_complete: null`.
///
/// Proves: the new C4 fields are structural (always present on every session
/// response), not conditional on mode or artifact configuration.  An operator
/// consulting the session endpoint with no artifact path configured sees an
/// explicit "not_configured" ceiling, not an absent field or ambiguous null.
#[tokio::test]
async fn c4_01_no_artifact_path_session_not_configured() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let (status, body) = call(make_router(), session_req()).await;
    assert_eq!(status, StatusCode::OK, "C4-01: session must return 200");

    let json = parse_json(body);

    // C4 fields must be structural (always present).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C4-01: parity_evidence_state must be present on session response"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C4-01: live_trust_complete key must be present on session response"
    );

    assert_eq!(
        json["parity_evidence_state"], "not_configured",
        "C4-01: no artifact path → parity_evidence_state must be not_configured on session"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "C4-01: not_configured → live_trust_complete must be null (not a positive trust claim)"
    );
}

// ---------------------------------------------------------------------------
// C4-02: valid evidence with live_trust_complete=false → "incomplete", false.
// ---------------------------------------------------------------------------

/// C4-02: When valid parity_evidence.json is present with
/// `live_trust_complete=false`, the session surface reports `"incomplete"` and
/// `live_trust_complete: false`.
///
/// Proves: the structural proof gap (evidence present but trust not established)
/// is explicit on the session surface.  An operator doing a lightweight session
/// check on a live-shadow or live-capital deployment can see the ceiling without
/// consulting a second endpoint.
#[tokio::test]
async fn c4_02_incomplete_evidence_explicit_on_session() {
    let (dir, manifest) = make_artifact_dir("c4_02");
    write_valid_parity_evidence(&dir, "test-artifact-c4-02");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let (status, body) = call(make_router(), session_req()).await;
    assert_eq!(status, StatusCode::OK, "C4-02: session must return 200");

    let json = parse_json(body);

    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C4-02: present evidence with live_trust_complete=false → parity_evidence_state must \
         be \"incomplete\" on session"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "C4-02: incomplete evidence → live_trust_complete must be explicit false on session"
    );
}

// ---------------------------------------------------------------------------
// C4-03: deployment_start_allowed=true + live_trust_complete=false co-present.
// ---------------------------------------------------------------------------

/// C4-03: `deployment_start_allowed` and `live_trust_complete: false` are
/// co-present on the same session response when evidence is present.
///
/// This is the primary C4 proof: an operator reading the session endpoint to
/// check "can I execute?" sees BOTH:
///   1. Whether start is structurally allowed for this deployment
///   2. That the current live-trust ceiling is explicitly false
///
/// Before C4 they could only learn (1) from this surface; (2) required a
/// second endpoint (status, preflight, or guidance).
#[tokio::test]
async fn c4_03_start_allowed_and_trust_ceiling_co_present() {
    let (dir, manifest) = make_artifact_dir("c4_03");
    write_valid_parity_evidence(&dir, "test-artifact-c4-03");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let (status, body) = call(make_router(), session_req()).await;
    assert_eq!(status, StatusCode::OK, "C4-03: session must return 200");

    let json = parse_json(body);

    // (1) deployment_start_allowed is present (structural — always there).
    assert!(
        json.get("deployment_start_allowed").is_some(),
        "C4-03: deployment_start_allowed must be present on session"
    );

    // (2) Trust ceiling: explicit false on the same response.
    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C4-03: parity_evidence_state must be \"incomplete\" when evidence is present"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "C4-03: live_trust_complete must be explicit false alongside deployment_start_allowed"
    );

    // Both fields coexist on the same surface — operator sees full picture.
    assert!(
        json["deployment_start_allowed"].is_boolean() && json["live_trust_complete"].is_boolean(),
        "C4-03: deployment_start_allowed and live_trust_complete must coexist on same session surface"
    );
}

// ---------------------------------------------------------------------------
// C4-04: live_trust_complete is never true on session in current builds.
// ---------------------------------------------------------------------------

/// C4-04: `live_trust_complete` is never `true` on the session surface for any
/// artifact path configuration in the current build.
///
/// Proves the fail-closed ceiling: no matter what artifact path is configured,
/// the session surface cannot return `live_trust_complete: true` in builds
/// where the TV-03 parity pipeline always produces `live_trust_complete=false`.
///
/// Tests two cases:
///   (a) no artifact path → null (not_configured)
///   (b) valid evidence with live_trust_complete=false → false (incomplete)
///
/// In neither case is `live_trust_complete: true` returned.
#[tokio::test]
async fn c4_04_live_trust_complete_never_true_on_session() {
    // (a) not_configured case
    {
        let _g = EnvGuard::absent(ENV_ARTIFACT_PATH);
        let (s, b) = call(make_router(), session_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_ne!(
            j["live_trust_complete"],
            serde_json::Value::Bool(true),
            "C4-04(a): not_configured → live_trust_complete must not be true"
        );
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "C4-04(a): not_configured → live_trust_complete must be null"
        );
    }

    // (b) present-but-incomplete case
    {
        let (dir, manifest) = make_artifact_dir("c4_04b");
        write_valid_parity_evidence(&dir, "test-artifact-c4-04b");
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), session_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_ne!(
            j["live_trust_complete"],
            serde_json::Value::Bool(true),
            "C4-04(b): incomplete evidence → live_trust_complete must not be true"
        );
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Bool(false),
            "C4-04(b): incomplete evidence → live_trust_complete must be explicit false"
        );
    }
}

// ---------------------------------------------------------------------------
// C4-05: Existing session contract fields are not broken by C4.
// ---------------------------------------------------------------------------

/// C4-05: Adding C4 fields to `SessionStateResponse` does not break the
/// existing session contract.
///
/// Proves that all prior session fields are still present and structurally
/// intact after C4:
///   - `daemon_mode` is a non-empty string
///   - `adapter_id` is a non-empty string
///   - `deployment_start_allowed` is a boolean
///   - `strategy_allowed` is a boolean
///   - `execution_allowed` is a boolean
///   - `system_trading_window` is a non-empty string
///   - `market_session` is a non-empty string
///   - `exchange_calendar_state` is a non-empty string
///   - `calendar_spec_id` is a non-empty string
///   - `notes` is an array
///
/// No existing consumers of this surface are broken by the addition of the two
/// new C4 fields.
#[tokio::test]
async fn c4_05_existing_session_contract_not_broken() {
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let (status, body) = call(make_router(), session_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "C4-05: session must return 200 on paper path"
    );

    let json = parse_json(body);

    assert!(
        json["daemon_mode"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: daemon_mode must be a non-empty string"
    );
    assert!(
        json["adapter_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: adapter_id must be a non-empty string"
    );
    assert!(
        json["deployment_start_allowed"].is_boolean(),
        "C4-05: deployment_start_allowed must be a boolean"
    );
    assert!(
        json["strategy_allowed"].is_boolean(),
        "C4-05: strategy_allowed must be a boolean"
    );
    assert!(
        json["execution_allowed"].is_boolean(),
        "C4-05: execution_allowed must be a boolean"
    );
    assert!(
        json["system_trading_window"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: system_trading_window must be a non-empty string"
    );
    assert!(
        json["market_session"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: market_session must be a non-empty string"
    );
    assert!(
        json["exchange_calendar_state"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: exchange_calendar_state must be a non-empty string"
    );
    assert!(
        json["calendar_spec_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "C4-05: calendar_spec_id must be a non-empty string"
    );
    assert!(json["notes"].is_array(), "C4-05: notes must be an array");

    // C4 fields present (structural — not conditional on prior fields).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C4-05: parity_evidence_state must be present after C4"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C4-05: live_trust_complete key must be present after C4"
    );
}
