//! C3: Live-trust truth on the mode-change-guidance surface — proof tests.
//!
//! ## What C3 closes
//!
//! Before C3, `GET /api/v1/ops/mode-change-guidance` gave an operator:
//!   - `transition_verdicts` showing Paper→LiveShadow = "admissible_with_restart"
//!   - preconditions listing "provide parity evidence (TV-03)"
//!   - no signal about whether parity evidence is already present on this
//!     deployment and what `live_trust_complete` currently is
//!
//! An operator consulting mode-change-guidance to plan a Paper→LiveShadow
//! transition would see that they need parity evidence without seeing whether:
//!   (a) evidence is absent (they need to produce it), or
//!   (b) evidence is present but `live_trust_complete=false` (a structural
//!       proof gap in the current build — no operator action can lift it without
//!       an explicit proof patch)
//!
//! To distinguish (a) from (b), the operator had to consult a second surface
//! (`/api/v1/system/status` or `/api/v1/system/parity-evidence`).  An operator
//! who only read mode-change-guidance could mistake case (b) for case (a) and
//! spend effort trying to produce or locate evidence that already exists.  More
//! importantly, they could not read the current live-trust ceiling on the same
//! surface they use for mode-transition planning.
//!
//! C3 adds the same two trust-ceiling fields from C1/C2 directly to
//! `ModeChangeGuidanceResponse`:
//!   - `parity_evidence_state` — current evidence state label (same enum as C1/C2)
//!   - `live_trust_complete` — same semantics as C1/C2; null is never a
//!     positive trust claim; `Some(false)` is the explicit ceiling in current builds
//!
//! The same `evaluate_parity_evidence_guarded()` evaluator is used across all
//! four surfaces (status/C1, preflight/C2, parity-evidence, guidance/C3) so
//! they cannot diverge.
//!
//! ## Tests (all pure in-process; env-var races serialised via `ENV_LOCK`)
//!
//! - C3-01: no `MQK_ARTIFACT_PATH` → guidance `parity_evidence_state:
//!   "not_configured"`, `live_trust_complete: null`; not a positive trust
//!   claim.
//! - C3-02: valid evidence with `live_trust_complete=false` → guidance
//!   `"incomplete"`, `live_trust_complete: false`; explicit honest ceiling
//!   alongside the Paper→LiveShadow admissibility verdict on same surface.
//! - C3-03: Paper→LiveShadow "admissible_with_restart" + explicit
//!   `live_trust_complete: false` co-present on the same surface.  Proves
//!   the gap between "transition is structurally admissible" and "trust
//!   ceiling is not met" is explicitly visible to the operator.
//! - C3-04: `live_trust_complete` is never true on the guidance surface in
//!   the current build for any artifact path configuration.
//! - C3-05: Existing paper-path contract (transition_verdicts, transition_permitted,
//!   operator_next_steps, restart_workflow) is not broken by C3.

use std::io::Write as _;
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ENV_ARTIFACT_PATH: &str = "MQK_ARTIFACT_PATH";

// ---------------------------------------------------------------------------
// Env-var serialisation — same pattern as scenario_artifact_deployability_tv02.rs
// ---------------------------------------------------------------------------

/// Serialises tests that mutate `MQK_ARTIFACT_PATH` so they do not race.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

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

fn guidance_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// RAII guard: saves and clears an env var; restores on drop.
/// Caller must hold `env_lock()` for the duration of the guard's lifetime.
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

/// Create a unique temp dir with an empty promoted_manifest.json.
fn make_artifact_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mqk_c3_{tag}_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).expect("create manifest");
    (dir, manifest)
}

// ---------------------------------------------------------------------------
// C3-01: no MQK_ARTIFACT_PATH → guidance "not_configured", live_trust_complete null.
// ---------------------------------------------------------------------------

/// C3-01: Without MQK_ARTIFACT_PATH the mode-change-guidance surface reports
/// `parity_evidence_state: "not_configured"` and `live_trust_complete: null`.
///
/// Proves: the new C3 fields are structural (always present), not conditional.
/// An operator consulting guidance on a deployment with no artifact path sees an
/// explicit "not_configured" ceiling, not an absent field or ambiguous null.
#[tokio::test]
async fn c3_01_no_artifact_path_guidance_not_configured() {
    let _lock = env_lock().lock().await;
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, guidance_req()).await;
    assert_eq!(status, StatusCode::OK, "C3-01: guidance must return 200");

    let json = parse_json(body);

    // C3 fields must be structural (always present).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C3-01: parity_evidence_state must be present on guidance response"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C3-01: live_trust_complete key must be present on guidance response"
    );

    assert_eq!(
        json["parity_evidence_state"], "not_configured",
        "C3-01: no artifact path → parity_evidence_state must be not_configured on guidance"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Null,
        "C3-01: not_configured → live_trust_complete must be null (not a positive trust claim)"
    );
}

// ---------------------------------------------------------------------------
// C3-02: valid evidence with live_trust_complete=false → "incomplete", false.
// ---------------------------------------------------------------------------

/// C3-02: When valid parity_evidence.json is present with
/// `live_trust_complete=false`, guidance reports `"incomplete"` and
/// `live_trust_complete: false`.
///
/// Proves: the structural proof gap (evidence present but trust not established)
/// is explicit on the mode-change-guidance surface.  An operator planning a
/// mode transition can see the ceiling without consulting a second endpoint.
#[tokio::test]
async fn c3_02_incomplete_evidence_explicit_on_guidance() {
    let _lock = env_lock().lock().await;
    let (dir, manifest) = make_artifact_dir("c3_02");
    write_valid_parity_evidence(&dir, "test-artifact-c3-02");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, guidance_req()).await;
    assert_eq!(status, StatusCode::OK, "C3-02: guidance must return 200");

    let json = parse_json(body);

    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C3-02: present evidence with live_trust_complete=false → parity_evidence_state must \
         be \"incomplete\" on guidance"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "C3-02: incomplete evidence → live_trust_complete must be explicit false on guidance"
    );
}

// ---------------------------------------------------------------------------
// C3-03: admissible_with_restart + live_trust_complete=false co-present.
// ---------------------------------------------------------------------------

/// C3-03: Paper→LiveShadow is "admissible_with_restart" in transition_verdicts
/// AND `live_trust_complete: false` is present on the same surface.
///
/// This is the primary C3 proof: an operator reading mode-change-guidance to
/// plan a Paper→LiveShadow transition sees BOTH:
///   1. That the transition is structurally admissible (good to know)
///   2. That the current live-trust ceiling is explicitly false (critical to know)
///
/// Without C3 they could only learn (1) from this surface and would need a
/// second endpoint to learn (2).
#[tokio::test]
async fn c3_03_live_shadow_admissible_and_trust_ceiling_co_present() {
    let _lock = env_lock().lock().await;
    let (dir, manifest) = make_artifact_dir("c3_03");
    write_valid_parity_evidence(&dir, "test-artifact-c3-03");
    let _guard = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());

    let router = make_router();
    let (status, body) = call(router, guidance_req()).await;
    assert_eq!(status, StatusCode::OK, "C3-03: guidance must return 200");

    let json = parse_json(body);

    // (1) Transition verdict: Paper→LiveShadow is admissible_with_restart.
    let verdicts = json["transition_verdicts"]
        .as_array()
        .expect("C3-03: transition_verdicts must be an array");
    let live_shadow_entry = verdicts
        .iter()
        .find(|e| e["target_mode"].as_str() == Some("live-shadow"))
        .expect("C3-03: must have a live-shadow entry in transition_verdicts");
    assert_eq!(
        live_shadow_entry["verdict"].as_str(),
        Some("admissible_with_restart"),
        "C3-03: Paper→LiveShadow must be admissible_with_restart"
    );

    // (2) Trust ceiling: explicit false on the same response.
    assert_eq!(
        json["parity_evidence_state"], "incomplete",
        "C3-03: parity_evidence_state must be \"incomplete\" when evidence is present"
    );
    assert_eq!(
        json["live_trust_complete"],
        serde_json::Value::Bool(false),
        "C3-03: live_trust_complete must be explicit false alongside admissible_with_restart"
    );

    // Both fields coexist in the same JSON body — operator sees full picture.
    assert!(
        json["transition_verdicts"].is_array() && json["live_trust_complete"].is_boolean(),
        "C3-03: transition_verdicts and live_trust_complete must coexist on same guidance surface"
    );
}

// ---------------------------------------------------------------------------
// C3-04: live_trust_complete is never true on guidance in current builds.
// ---------------------------------------------------------------------------

/// C3-04: `live_trust_complete` is never `true` on the mode-change-guidance
/// surface for any artifact path configuration in the current build.
///
/// Proves the fail-closed ceiling: no matter what artifact path is configured,
/// the guidance surface cannot return `live_trust_complete: true` in builds
/// where the TV-03 parity pipeline always produces `live_trust_complete=false`.
///
/// Tests two cases:
///   (a) no artifact path → null (not_configured)
///   (b) valid evidence with live_trust_complete=false → false (incomplete)
///
/// In neither case is `live_trust_complete: true` returned.
#[tokio::test]
async fn c3_04_live_trust_complete_never_true_on_guidance() {
    let _lock = env_lock().lock().await;
    // (a) not_configured case
    {
        let _g = EnvGuard::absent(ENV_ARTIFACT_PATH);
        let (s, b) = call(make_router(), guidance_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_ne!(
            j["live_trust_complete"],
            serde_json::Value::Bool(true),
            "C3-04(a): not_configured → live_trust_complete must not be true"
        );
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Null,
            "C3-04(a): not_configured → live_trust_complete must be null"
        );
    }

    // (b) present-but-incomplete case
    {
        let (dir, manifest) = make_artifact_dir("c3_04b");
        write_valid_parity_evidence(&dir, "test-artifact-c3-04b");
        let _g = EnvGuard::set(ENV_ARTIFACT_PATH, manifest.to_str().unwrap());
        let (s, b) = call(make_router(), guidance_req()).await;
        assert_eq!(s, StatusCode::OK);
        let j = parse_json(b);
        assert_ne!(
            j["live_trust_complete"],
            serde_json::Value::Bool(true),
            "C3-04(b): incomplete evidence → live_trust_complete must not be true"
        );
        assert_eq!(
            j["live_trust_complete"],
            serde_json::Value::Bool(false),
            "C3-04(b): incomplete evidence → live_trust_complete must be explicit false"
        );
    }
}

// ---------------------------------------------------------------------------
// C3-05: Existing paper-path contract fields are not broken by C3.
// ---------------------------------------------------------------------------

/// C3-05: Adding C3 fields to `ModeChangeGuidanceResponse` does not break the
/// existing paper-path contract.
///
/// Proves that all prior CC-03A/CC-03B/CC-03C fields are still present and
/// structurally intact after C3:
///   - `transition_permitted` == false (hot switching universally refused)
///   - `transition_verdicts` has 4 entries with correct Paper-mode verdicts
///   - `operator_next_steps` is non-empty
///   - `restart_workflow` is present
///   - `canonical_route` identifies the correct endpoint
///
/// No existing consumers of this surface are broken by the addition of the
/// two new C3 fields.
#[tokio::test]
async fn c3_05_existing_paper_contract_not_broken() {
    let _lock = env_lock().lock().await;
    let _guard = EnvGuard::absent(ENV_ARTIFACT_PATH);

    let router = make_router();
    let (status, body) = call(router, guidance_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "C3-05: guidance must return 200 on paper path"
    );

    let json = parse_json(body);

    // canonical_route identifies the endpoint.
    assert_eq!(
        json["canonical_route"].as_str(),
        Some("/api/v1/ops/mode-change-guidance"),
        "C3-05: canonical_route must identify the guidance endpoint"
    );

    // transition_permitted is always false.
    assert_eq!(
        json["transition_permitted"],
        serde_json::Value::Bool(false),
        "C3-05: transition_permitted must still be false"
    );

    // transition_verdicts has 4 entries with correct Paper-mode verdicts.
    let verdicts = json["transition_verdicts"]
        .as_array()
        .expect("C3-05: transition_verdicts must be an array");
    assert_eq!(
        verdicts.len(),
        4,
        "C3-05: transition_verdicts must have 4 entries; got: {verdicts:?}"
    );

    let find = |target: &str| {
        verdicts
            .iter()
            .find(|e| e["target_mode"].as_str() == Some(target))
            .unwrap_or_else(|| panic!("C3-05: no entry for target_mode={target}"))
    };

    assert_eq!(
        find("paper")["verdict"].as_str(),
        Some("same_mode"),
        "C3-05: Paper→Paper"
    );
    assert_eq!(
        find("live-shadow")["verdict"].as_str(),
        Some("admissible_with_restart"),
        "C3-05: Paper→LiveShadow"
    );
    assert_eq!(
        find("live-capital")["verdict"].as_str(),
        Some("fail_closed"),
        "C3-05: Paper→LiveCapital"
    );
    assert_eq!(
        find("backtest")["verdict"].as_str(),
        Some("refused"),
        "C3-05: Paper→Backtest"
    );

    // operator_next_steps is non-empty.
    assert!(
        json["operator_next_steps"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "C3-05: operator_next_steps must be non-empty"
    );

    // restart_workflow is present.
    assert!(
        json.get("restart_workflow").is_some(),
        "C3-05: restart_workflow must still be present after C3"
    );

    // C3 fields present on paper path (structural, not conditional on mode).
    assert!(
        json.get("parity_evidence_state").is_some(),
        "C3-05: parity_evidence_state must be present on paper path"
    );
    assert!(
        json.get("live_trust_complete").is_some(),
        "C3-05: live_trust_complete key must be present on paper path"
    );
}
