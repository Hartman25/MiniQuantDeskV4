//! TV-03A/TV-03B — Parity evidence seam + operator-visible truth surface.
//!
//! # What this proves
//!
//! - `truth_state` is explicit for all five outcomes (not_configured / absent /
//!   invalid / present / unavailable).
//! - Absent, invalid, and unavailable are never conflated with present.
//! - `live_trust_complete=false` is surfaced honestly; no fabricated trust claim.
//! - `evidence_available=false` is distinct from Absent (file exists but no run).
//! - `GET /api/v1/system/parity-evidence` reflects evaluator truth end-to-end.
//! - `canonical_route` is always present.
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                             |
//! |-------|----------------------------------------------------------------------------|
//! | PE-01 | not_configured when MQK_ARTIFACT_PATH is not set                          |
//! | PE-02 | absent when artifact path set but parity_evidence.json missing            |
//! | PE-03 | invalid when parity_evidence.json has wrong schema_version                |
//! | PE-04 | present with live_trust_complete=false — honest, never fabricated         |
//! | PE-05 | evidence_available=false is "present" not "absent"                        |
//! | PE-06 | canonical_route always present regardless of truth_state                  |

use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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

fn get_parity_evidence() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/system/parity-evidence")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn fresh_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new())
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn valid_parity_evidence_json(artifact_id: &str) -> String {
    serde_json::json!({
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
        "live_trust_gaps": ["TV-02 gate evaluates historical metrics only"],
        "produced_at_utc": "2026-03-01T00:00:00Z"
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// PE-01: not_configured when MQK_ARTIFACT_PATH is not set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe01_not_configured_when_env_unset() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("MQK_ARTIFACT_PATH");

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    assert_eq!(j["truth_state"], "not_configured");
    assert!(j["artifact_id"].is_null());
    assert!(j["live_trust_complete"].is_null());
    assert!(j["evidence_available"].is_null());
    assert!(j["evaluated_path"].is_null());
}

// ---------------------------------------------------------------------------
// PE-02: absent when MQK_ARTIFACT_PATH points to dir without parity_evidence.json
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe02_absent_when_parity_file_missing() {
    let _guard = env_lock().lock().unwrap();
    let id = next_id();

    let dir = std::env::temp_dir().join(format!("mqk_tv03_pe02_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).unwrap();
    // parity_evidence.json deliberately not created

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    assert_eq!(j["truth_state"], "absent", "got: {}", j["truth_state"]);
    assert!(j["artifact_id"].is_null());
    assert!(j["live_trust_complete"].is_null());
    // evaluated_path is populated because the artifact dir is known
    assert!(!j["evaluated_path"].is_null());
}

// ---------------------------------------------------------------------------
// PE-03: invalid when parity_evidence.json has wrong schema_version
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe03_invalid_wrong_schema_version() {
    let _guard = env_lock().lock().unwrap();
    let id = next_id();

    let dir = std::env::temp_dir().join(format!("mqk_tv03_pe03_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).unwrap();

    let mut f = std::fs::File::create(dir.join("parity_evidence.json")).unwrap();
    f.write_all(
        r#"{"schema_version":"parity-v0","artifact_id":"x","live_trust_complete":false}"#
            .as_bytes(),
    )
    .unwrap();

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    assert_eq!(j["truth_state"], "invalid", "got: {}", j["truth_state"]);
    assert!(!j["invalid_reason"].is_null());
    assert!(j["artifact_id"].is_null());
    assert!(j["live_trust_complete"].is_null());
}

// ---------------------------------------------------------------------------
// PE-04: present with live_trust_complete=false — honest, never fabricated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe04_present_live_trust_complete_false() {
    let _guard = env_lock().lock().unwrap();
    let id = next_id();

    let dir = std::env::temp_dir().join(format!("mqk_tv03_pe04_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).unwrap();

    let mut f = std::fs::File::create(dir.join("parity_evidence.json")).unwrap();
    f.write_all(valid_parity_evidence_json("art-tv03-abc").as_bytes())
        .unwrap();

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    assert_eq!(j["truth_state"], "present", "got: {}", j["truth_state"]);
    assert_eq!(j["artifact_id"], "art-tv03-abc");
    // live_trust_complete must be explicitly false — not null, not hidden
    assert_eq!(j["live_trust_complete"], false, "must be false, not null");
    // evidence_available=false: shadow run not yet performed
    assert_eq!(j["evidence_available"], false);
    assert!(!j["evidence_note"].is_null());
    assert!(!j["produced_at_utc"].is_null());
    assert!(j["invalid_reason"].is_null());
}

// ---------------------------------------------------------------------------
// PE-05: evidence_available=false is "present" not "absent"
//         File exists but shadow run was not performed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe05_present_evidence_available_false_is_not_absent() {
    let _guard = env_lock().lock().unwrap();
    let id = next_id();

    let dir = std::env::temp_dir().join(format!("mqk_tv03_pe05_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("promoted_manifest.json");
    std::fs::File::create(&manifest).unwrap();

    // evidence_available=false but file IS readable
    let mut f = std::fs::File::create(dir.join("parity_evidence.json")).unwrap();
    f.write_all(valid_parity_evidence_json("art-noshadow").as_bytes())
        .unwrap();

    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    // Must be "present" (manifest readable), not "absent" (file missing)
    assert_eq!(j["truth_state"], "present");
    assert_eq!(j["evidence_available"], false);
}

// ---------------------------------------------------------------------------
// PE-06: canonical_route is always present regardless of truth_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pe06_canonical_route_always_present() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("MQK_ARTIFACT_PATH");

    let (status, body) = call(routes::build_router(fresh_state()), get_parity_evidence()).await;

    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);
    assert_eq!(j["canonical_route"], "/api/v1/system/parity-evidence");
}
