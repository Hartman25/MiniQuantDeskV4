//! MULTI-SYMBOL-CAPS-PREFLIGHT-WARNING-01: advisory (non-blocking) preflight
//! warning when any of caps #2/#3/#5 are unset/disabled.
//!
//! `scenario_multi_symbol_capital_caps_01.rs` already confirms caps #2
//! (`MQK_PER_SYMBOL_MAX_POSITION_QTY`), #3 (`MQK_PER_SYMBOL_MAX_NOTIONAL_USD`),
//! and #5 (`MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD`) all default to
//! `None`/disabled at the enforcement layer. This file proves the operator
//! actually sees that fact on `GET /api/v1/system/preflight` — via the
//! existing `warnings` field, never a new blocker and never a change to
//! `deployment_start_allowed`.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                        |
//! |------|------------------------------------------------------------------------|
//! | PW01 | All three caps set → no cap warnings                                  |
//! | PW02 | Only cap #2 missing → warning names cap #2 only                       |
//! | PW03 | Only cap #3 missing → warning names cap #3 only                       |
//! | PW04 | Only cap #5 missing → warning names cap #5 only                       |
//! | PW05 | All three missing → warning names all three                           |
//! | PW06 | `deployment_start_allowed` identical whether caps are set or unset    |
//! | PW07 | Cap warnings never appear in `blockers`, only in `warnings`           |

use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;
use tower::ServiceExt;

const ENV_CAP2: &str = "MQK_PER_SYMBOL_MAX_POSITION_QTY";
const ENV_CAP3: &str = "MQK_PER_SYMBOL_MAX_NOTIONAL_USD";
const ENV_CAP5: &str = "MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD";

// ---------------------------------------------------------------------------
// Env-var serialisation — same pattern as scenario_preflight_live_trust_c2.rs
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII guard: saves and clears/sets an env var; restores on drop.
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

// ---------------------------------------------------------------------------
// HTTP helpers
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

fn warnings_text(body: &serde_json::Value) -> String {
    body["warnings"]
        .as_array()
        .expect("warnings must be an array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" | ")
}

// ---------------------------------------------------------------------------
// PW01 — all three caps set → no cap warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw01_all_three_caps_set_no_cap_warnings() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::set(ENV_CAP2, "100");
    let _g3 = EnvGuard::set(ENV_CAP3, "50000");
    let _g5 = EnvGuard::set(ENV_CAP5, "250000");

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let text = warnings_text(&json);

    assert!(!text.contains(ENV_CAP2), "unexpected cap #2 warning: {text}");
    assert!(!text.contains(ENV_CAP3), "unexpected cap #3 warning: {text}");
    assert!(!text.contains(ENV_CAP5), "unexpected cap #5 warning: {text}");
}

// ---------------------------------------------------------------------------
// PW02 — only cap #2 missing → warning names cap #2 only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw02_only_cap2_missing_names_cap2_only() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::absent(ENV_CAP2);
    let _g3 = EnvGuard::set(ENV_CAP3, "50000");
    let _g5 = EnvGuard::set(ENV_CAP5, "250000");

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let text = warnings_text(&json);

    assert!(text.contains(ENV_CAP2), "expected cap #2 warning, got: {text}");
    assert!(!text.contains(ENV_CAP3), "unexpected cap #3 warning: {text}");
    assert!(!text.contains(ENV_CAP5), "unexpected cap #5 warning: {text}");
}

// ---------------------------------------------------------------------------
// PW03 — only cap #3 missing → warning names cap #3 only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw03_only_cap3_missing_names_cap3_only() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::set(ENV_CAP2, "100");
    let _g3 = EnvGuard::absent(ENV_CAP3);
    let _g5 = EnvGuard::set(ENV_CAP5, "250000");

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let text = warnings_text(&json);

    assert!(!text.contains(ENV_CAP2), "unexpected cap #2 warning: {text}");
    assert!(text.contains(ENV_CAP3), "expected cap #3 warning, got: {text}");
    assert!(!text.contains(ENV_CAP5), "unexpected cap #5 warning: {text}");
}

// ---------------------------------------------------------------------------
// PW04 — only cap #5 missing → warning names cap #5 only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw04_only_cap5_missing_names_cap5_only() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::set(ENV_CAP2, "100");
    let _g3 = EnvGuard::set(ENV_CAP3, "50000");
    let _g5 = EnvGuard::absent(ENV_CAP5);

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let text = warnings_text(&json);

    assert!(!text.contains(ENV_CAP2), "unexpected cap #2 warning: {text}");
    assert!(!text.contains(ENV_CAP3), "unexpected cap #3 warning: {text}");
    assert!(text.contains(ENV_CAP5), "expected cap #5 warning, got: {text}");
}

// ---------------------------------------------------------------------------
// PW05 — all three missing → warning names all three
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw05_all_three_missing_names_all_three() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::absent(ENV_CAP2);
    let _g3 = EnvGuard::absent(ENV_CAP3);
    let _g5 = EnvGuard::absent(ENV_CAP5);

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let text = warnings_text(&json);

    assert!(text.contains(ENV_CAP2), "expected cap #2 warning, got: {text}");
    assert!(text.contains(ENV_CAP3), "expected cap #3 warning, got: {text}");
    assert!(text.contains(ENV_CAP5), "expected cap #5 warning, got: {text}");
}

// ---------------------------------------------------------------------------
// PW06 — deployment_start_allowed identical whether caps are set or unset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw06_start_allowed_identical_regardless_of_cap_state() {
    let _lock = env_lock().lock().await;

    let _g2 = EnvGuard::set(ENV_CAP2, "100");
    let _g3 = EnvGuard::set(ENV_CAP3, "50000");
    let _g5 = EnvGuard::set(ENV_CAP5, "250000");
    let (_, body_set) = call(make_router(), preflight_req()).await;
    let json_set = parse_json(body_set);

    drop((_g2, _g3, _g5));
    let _g2 = EnvGuard::absent(ENV_CAP2);
    let _g3 = EnvGuard::absent(ENV_CAP3);
    let _g5 = EnvGuard::absent(ENV_CAP5);
    let (_, body_unset) = call(make_router(), preflight_req()).await;
    let json_unset = parse_json(body_unset);

    assert_eq!(
        json_set["deployment_start_allowed"], json_unset["deployment_start_allowed"],
        "advisory cap warnings must never change deployment_start_allowed"
    );
    assert_eq!(
        json_set["blockers"], json_unset["blockers"],
        "advisory cap warnings must never add/remove a blocker"
    );
}

// ---------------------------------------------------------------------------
// PW07 — cap warnings never appear in blockers, only in warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pw07_cap_warnings_never_appear_in_blockers() {
    let _lock = env_lock().lock().await;
    let _g2 = EnvGuard::absent(ENV_CAP2);
    let _g3 = EnvGuard::absent(ENV_CAP3);
    let _g5 = EnvGuard::absent(ENV_CAP5);

    let (status, body) = call(make_router(), preflight_req()).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    let blockers_text = json["blockers"]
        .as_array()
        .expect("blockers must be an array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" | ");

    assert!(!blockers_text.contains(ENV_CAP2));
    assert!(!blockers_text.contains(ENV_CAP3));
    assert!(!blockers_text.contains(ENV_CAP5));
}
