//! LO-03F — Shadow-to-live cutover proof with parity evidence requirement.
//!
//! # What this proves
//!
//! - Shadow-to-live cutover (LiveShadow → LiveCapital) is not permissive or implicit:
//!   the canonical mode-transition seam returns `fail_closed` for this pair.
//! - The operator-visible truth surface (`GET /api/v1/ops/mode-change-guidance`) from
//!   a LiveShadow daemon surfaces the LiveCapital target as `fail_closed` with a reason
//!   that explicitly references the parity chain / `live_trust_complete`.
//! - Parity evidence is a hard requirement for LiveCapital start: absent or invalid
//!   parity evidence blocks start at the TV-03C gate with `gate=parity_evidence`.
//! - No synthetic success path exists when parity evidence is missing or invalid:
//!   the gate name is explicit and the 403 is unambiguous.
//! - When parity evidence IS present (`live_trust_complete=false` in current TV-03),
//!   the start gate does not fabricate an additional block — it proceeds honestly to
//!   the DB gate.  The mode_transition guidance is the advisory layer; the TV-03C
//!   parity gate is the enforcement layer for evidence existence.
//!
//! # Gate ordering context for F03/F04/F05
//!
//! At the start boundary (armed LiveCapital+Alpaca, WS=Live, TokenRequired):
//!   1. deployment_readiness   → passes (LiveCapital+Alpaca)
//!   2. integrity              → passes (armed)
//!   3. operator_auth          → passes (TokenRequired, valid bearer token)
//!   4. WS continuity          → passes (Live state forced in test setup)
//!   5. TV-02C artifact gate   → passes (manifest + gate file present)
//!   6. TV-03C parity gate     ← what F03/F04/F05 exercise
//!   7. TV-04A capital policy  → not triggered (no MQK_CAPITAL_POLICY_PATH)
//!   8. db_pool()              → ServiceUnavailable (no DB in test) → 503
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                      |
//! |------|-------------------------------------------------------------------------------------|
//! | F01  | LiveShadow→LiveCapital: mode_transition returns fail_closed, reason references parity|
//! | F02  | GET mode-change-guidance from LiveShadow daemon: LiveCapital target = fail_closed    |
//! | F03  | LiveCapital+armed+WS=Live + absent parity → 403 gate=parity_evidence               |
//! | F04  | LiveCapital+armed+WS=Live + invalid parity → 403 gate=parity_evidence              |
//! | F05  | LiveCapital+armed+WS=Live + parity present → proceeds to DB gate (503, not 403)    |
//!
//! # Does not reopen
//!
//! - LO-03A/LO-03B: live-shadow gate chain is closed
//! - LO-03C: restart / no-hot-switch proof is closed
//! - LO-03D/LO-03E: live-capital preflight and halt/disarm controls are closed
//! - LO-03G: arm/disarm audit durability is closed
//! - TV-03A/TV-03B/TV-03C: parity evidence seam, truth surface, and LiveShadow
//!   start boundary are closed (F03/F04/F05 prove the same gate applies for
//!   LiveCapital, not re-prove the LiveShadow path)
//!
//! All tests are pure in-process (no DB, no network, no real broker).
//! All tests are always runnable in CI without MQK_DATABASE_URL.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{mode_transition::evaluate_mode_transition, routes, state};
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

fn json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ---------------------------------------------------------------------------
// State helper: LiveCapital+Alpaca+TokenRequired+WS=Live+Armed
//
// Used by F03/F04/F05.  Gate ordering at start for this state:
//   1. deployment_readiness → passes (LiveCapital+Alpaca)
//   2. integrity            → passes (armed below)
//   3. operator_auth        → passes (TokenRequired set; token in requests)
//   4. WS continuity        → passes (Live forced below)
//   5. TV-02C artifact gate → depends on MQK_ARTIFACT_PATH (set by each test)
//   6. TV-03C parity gate   ← what F03/F04/F05 exercise
//   7. TV-04A capital gate  → not triggered (no MQK_CAPITAL_POLICY_PATH)
//   8. db_pool()            → 503 (no DB)
// ---------------------------------------------------------------------------

async fn armed_live_capital_state(token: &str) -> Arc<state::AppState> {
    let mut st_inner = state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveCapital,
        state::BrokerKind::Alpaca,
    );
    st_inner.operator_auth = state::OperatorAuthMode::TokenRequired(token.to_string());
    let st = Arc::new(st_inner);

    // Force WS continuity to Live so the live-capital WS continuity gate passes.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "lo03f-setup-msg".to_string(),
        last_event_at: "2026-03-29T00:00:00Z".to_string(),
    })
    .await;

    // Arm with the valid bearer token (operator router requires it for TokenRequired mode).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "test setup: arm must succeed");

    st
}

fn post_start_with_token(token: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

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

/// Write a temp artifact dir with manifest + deployability_gate + optional parity.
///
/// Returns `(manifest_path, dir)`.
fn write_artifact_dir(
    tag: &str,
    artifact_id: &str,
    parity_contents: Option<&str>,
) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_lo03f_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create artifact dir");
    let manifest = dir.join("promoted_manifest.json");
    std::fs::write(&manifest, valid_manifest(artifact_id)).expect("write manifest");
    std::fs::write(dir.join("deployability_gate.json"), gate_json_passed(artifact_id))
        .expect("write gate file");
    if let Some(parity) = parity_contents {
        std::fs::write(dir.join("parity_evidence.json"), parity).expect("write parity file");
    }
    (manifest, dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ===========================================================================
// F01 — mode_transition: LiveShadow→LiveCapital is fail_closed, reason
//        references the parity chain
//
// Pure test — no HTTP, no env vars, no FS.
// ===========================================================================

/// LO-03F / F01: `evaluate_mode_transition(LiveShadow, LiveCapital)` returns
/// `fail_closed`.  The reason must reference the parity chain / trust completeness
/// so operators understand why the cutover is blocked.
///
/// This proves:
/// - The canonical seam does not silently admit the shadow→capital transition.
/// - `fail_closed` is explicitly distinct from `admissible_with_restart`; no
///   precondition list can make this transition admissible in the current state.
/// - The reason string is honest about the parity proof gap.
#[test]
fn f01_live_shadow_to_live_capital_mode_transition_is_fail_closed() {
    let verdict = evaluate_mode_transition(
        state::DeploymentMode::LiveShadow,
        state::DeploymentMode::LiveCapital,
    );

    assert_eq!(
        verdict.as_str(),
        "fail_closed",
        "F01: LiveShadow→LiveCapital must be fail_closed; got: {:?}",
        verdict.as_str()
    );
    assert!(
        verdict.is_blocked(),
        "F01: fail_closed must be blocked (is_blocked=true)"
    );
    assert!(
        !verdict.is_admissible(),
        "F01: fail_closed must not be admissible (no precondition list unblocks this)"
    );

    let reason = verdict.reason();
    assert!(
        reason.contains("live_trust_complete")
            || reason.contains("parity")
            || reason.contains("proof"),
        "F01: fail_closed reason must reference the parity chain / live_trust_complete; \
         got: {reason:?}"
    );

    // Explicit: not the same-mode no-op
    assert_ne!(
        verdict.as_str(),
        "same_mode",
        "F01: LiveShadow→LiveCapital must not be same_mode"
    );
    // Explicit: not admissible_with_restart (no restart can make this admissible now)
    assert_ne!(
        verdict.as_str(),
        "admissible_with_restart",
        "F01: LiveShadow→LiveCapital must not be admissible_with_restart while parity proof \
         chain is incomplete"
    );
}

// ===========================================================================
// F02 — mode-change-guidance from LiveShadow daemon surfaces LiveCapital as
//        fail_closed with an honest, parity-referencing reason
// ===========================================================================

/// LO-03F / F02: `GET /api/v1/ops/mode-change-guidance` from a LiveShadow+Alpaca
/// daemon shows:
///
/// - `current_mode = "live-shadow"`
/// - `transition_permitted = false` (no hot switching)
/// - `transition_verdicts` includes a `live-capital` entry with
///   `verdict = "fail_closed"` and a reason mentioning parity / trust completeness
///
/// This proves the operator-visible truth surface is honest: it does not fabricate
/// a permissive path to live capital from shadow mode.
#[tokio::test]
async fn f02_mode_change_guidance_from_live_shadow_shows_live_capital_fail_closed() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::OK, "F02: mode-change-guidance must return 200");
    let j = json(body);

    assert_eq!(
        j["current_mode"].as_str().unwrap_or(""),
        "live-shadow",
        "F02: current_mode must be live-shadow"
    );
    assert_eq!(
        j["transition_permitted"].as_bool().unwrap_or(true),
        false,
        "F02: transition_permitted must always be false (no hot switching supported)"
    );

    // Find the live-capital entry in transition_verdicts.
    let verdicts = j["transition_verdicts"]
        .as_array()
        .expect("F02: transition_verdicts must be an array");
    let lc = verdicts
        .iter()
        .find(|v| v["target_mode"].as_str().unwrap_or("") == "live-capital")
        .expect("F02: transition_verdicts must include a live-capital entry");

    assert_eq!(
        lc["verdict"].as_str().unwrap_or(""),
        "fail_closed",
        "F02: live-capital target must have verdict=fail_closed from live-shadow; got: {lc}"
    );
    // Explicit: not admissible_with_restart
    assert_ne!(
        lc["verdict"].as_str().unwrap_or(""),
        "admissible_with_restart",
        "F02: live-capital target must NOT be admissible_with_restart while proof chain is \
         incomplete; got: {lc}"
    );

    let reason = lc["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("live_trust_complete")
            || reason.contains("parity")
            || reason.contains("fail-closed"),
        "F02: fail_closed reason must reference the parity chain / live_trust_complete; \
         got: {reason:?}"
    );

    // preconditions must be empty for fail_closed (no checklist unblocks it now)
    let preconditions = lc["preconditions"]
        .as_array()
        .expect("F02: preconditions must be an array");
    assert!(
        preconditions.is_empty(),
        "F02: fail_closed for live-capital must have no preconditions (no checklist path); \
         got: {preconditions:?}"
    );
}

// ===========================================================================
// F03 — LiveCapital+armed+WS=Live + absent parity → 403 gate=parity_evidence
//
// Proves that parity evidence is a hard requirement: absent evidence is not
// the same as parity proven.  The TV-03C gate applies to the LiveCapital
// start boundary, not only to LiveShadow.
// ===========================================================================

/// LO-03F / F03: When the daemon is in LiveCapital mode, all pre-parity gates
/// pass, but `parity_evidence.json` is absent, start is refused at the TV-03C
/// parity evidence gate with `gate=parity_evidence`.
///
/// Absent evidence ≠ parity proven.  Fail-closed.
///
/// Gate ordering:
///   deployment → pass, integrity → pass, operator_auth → pass,
///   WS continuity → pass, TV-02C artifact → pass,
///   TV-03C parity → BLOCKED here (absent)
#[tokio::test]
async fn f03_live_capital_start_absent_parity_blocked_at_parity_gate() {
    let _guard = env_lock().lock().unwrap();
    let token = "lo03f-f03-token";
    let artifact_id = "lo03f-f03-no-parity";

    // Artifact dir: manifest + deployability_gate, but NO parity_evidence.json
    let (manifest, dir) = write_artifact_dir("f03", artifact_id, None);
    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_capital_state(token).await;
    let (status, body) = call(
        routes::build_router(st),
        post_start_with_token(token),
    )
    .await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let j = json(body);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "F03: absent parity evidence must block LiveCapital start (403); body: {j}"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "F03: gate must be parity_evidence — absent parity evidence is not parity proven; body: {j}"
    );
    // Explicit: WS continuity gate passed (Live state was set)
    assert_ne!(
        j["gate"].as_str().unwrap_or(""),
        "alpaca_ws_continuity",
        "F03: WS continuity gate must have passed (Live state set); body: {j}"
    );
    // Explicit: operator_auth gate passed (TokenRequired with valid token)
    assert_ne!(
        j["gate"].as_str().unwrap_or(""),
        "operator_auth",
        "F03: operator_auth gate must have passed (TokenRequired set + valid token); body: {j}"
    );
    // Explicit: artifact_deployability gate passed (manifest + gate file present)
    assert_ne!(
        j["gate"].as_str().unwrap_or(""),
        "artifact_deployability",
        "F03: artifact_deployability gate must have passed (deployability_gate.json present); body: {j}"
    );
}

// ===========================================================================
// F04 — LiveCapital+armed+WS=Live + invalid parity → 403 gate=parity_evidence
//
// Proves that structurally invalid parity evidence is treated the same as
// absent evidence: fail-closed.  A corrupt or stale parity file is not a
// positive parity claim.
// ===========================================================================

/// LO-03F / F04: When the daemon is in LiveCapital mode, all pre-parity gates
/// pass, but `parity_evidence.json` has an invalid schema_version, start is
/// refused at the TV-03C parity evidence gate with `gate=parity_evidence`.
///
/// Invalid evidence ≠ parity proven.  Fail-closed.
#[tokio::test]
async fn f04_live_capital_start_invalid_parity_blocked_at_parity_gate() {
    let _guard = env_lock().lock().unwrap();
    let token = "lo03f-f04-token";
    let artifact_id = "lo03f-f04-invalid-parity";

    // Invalid parity: wrong schema_version — structurally invalid
    let invalid_parity = r#"{"schema_version":"parity-v0","artifact_id":"test"}"#;
    let (manifest, dir) = write_artifact_dir("f04", artifact_id, Some(invalid_parity));
    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = armed_live_capital_state(token).await;
    let (status, body) = call(
        routes::build_router(st),
        post_start_with_token(token),
    )
    .await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    cleanup(&dir);

    let j = json(body);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "F04: invalid parity evidence must block LiveCapital start (403); body: {j}"
    );
    assert_eq!(
        j["gate"].as_str().unwrap_or(""),
        "parity_evidence",
        "F04: gate must be parity_evidence — invalid parity evidence is not parity proven; body: {j}"
    );
    // Explicit: WS continuity gate passed
    assert_ne!(
        j["gate"].as_str().unwrap_or(""),
        "alpaca_ws_continuity",
        "F04: WS continuity gate must have passed; body: {j}"
    );
    // Explicit: operator_auth gate passed
    assert_ne!(
        j["gate"].as_str().unwrap_or(""),
        "operator_auth",
        "F04: operator_auth gate must have passed; body: {j}"
    );
}

// ===========================================================================
// F05 — LiveCapital+armed+WS=Live + parity present → 503 DB gate
//
// Proves two complementary truths:
//   (a) When evidence IS present, the parity gate does not fabricate an
//       additional block — it passes through to the next gate.
//   (b) live_trust_complete=false in the current TV-03 build does not cause
//       the TV-03C gate itself to block (the advisory layer is the
//       mode_transition verdict, not a secondary enforcement gate).
//
// Together, F03+F04+F05 prove: absent/invalid → blocked, present → not blocked.
// The mode_transition advisory layer (F01/F02) covers the "why this transition
// is not yet recommended" signal to the operator.
// ===========================================================================

/// LO-03F / F05: When the daemon is in LiveCapital mode, all pre-DB gates pass
/// (including parity evidence present with `live_trust_complete=false`), start
/// reaches and fires the DB gate (503 — no DB configured in test).
///
/// This proves:
/// - The TV-03C parity gate passes when evidence is present (not doubly blocked).
/// - The TV-04F live-capital policy gate passes when a policy is configured.
/// - The 503 is the definitive signal that all pre-DB gates were satisfied.
/// - No gate between the parity gate and db_pool() fabricates a new block.
///
/// Note: TV-04F (added) requires live-capital to have an explicit capital policy.
/// A minimal valid policy is configured in this test so TV-04F + TV-04A + TV-04D
/// all pass, and the test continues to prove the DB gate (503) is the final stop.
#[tokio::test]
async fn f05_live_capital_start_present_parity_proceeds_to_db_gate() {
    let _guard = env_lock().lock().unwrap();
    let token = "lo03f-f05-token";
    let artifact_id = "lo03f-f05-present-parity";

    let parity = valid_parity_json(artifact_id);
    let (manifest, dir) = write_artifact_dir("f05", artifact_id, Some(&parity));
    std::env::set_var("MQK_ARTIFACT_PATH", manifest.to_str().unwrap());

    // TV-04F: live-capital requires an explicit capital policy.
    // Write a minimal valid policy so TV-04F + TV-04A + TV-04D all pass.
    let policy_path = dir.join("capital_allocation_policy.json");
    std::fs::write(
        &policy_path,
        r#"{"schema_version":"policy-v1","policy_id":"lo03f-f05-policy","enabled":true,"max_portfolio_notional_usd":25000,"per_strategy_budgets":[]}"#,
    )
    .expect("F05: write capital policy file");
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &policy_path);

    let st = armed_live_capital_state(token).await;
    let (status, body) = call(
        routes::build_router(st),
        post_start_with_token(token),
    )
    .await;

    std::env::remove_var("MQK_ARTIFACT_PATH");
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");
    cleanup(&dir);

    let j = json(body);
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "F05: LiveCapital with present parity (live_trust_complete=false) must reach DB gate \
         (503); any 403 means a pre-DB gate was not satisfied; body: {j}"
    );
    // The 503 body identifies the DB as unavailable — all pre-DB gates passed.
    let error = j["error"].as_str().unwrap_or("");
    assert!(
        error.contains("runtime DB is not configured") || error.contains("DB"),
        "F05: 503 body must describe DB unavailability; got: {j}"
    );
    // Explicit: parity gate passed (evidence present)
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "parity_evidence",
        "F05: parity_evidence gate must have passed (evidence is present); body: {j}"
    );
    // Explicit: WS continuity gate passed (Live state was set)
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "alpaca_ws_continuity",
        "F05: WS continuity gate must have passed (Live state set); body: {j}"
    );
    // Explicit: operator_auth gate passed (TokenRequired with valid token)
    assert_ne!(
        j.get("gate").and_then(|g| g.as_str()).unwrap_or(""),
        "operator_auth",
        "F05: operator_auth gate must have passed (TokenRequired set); body: {j}"
    );
}
