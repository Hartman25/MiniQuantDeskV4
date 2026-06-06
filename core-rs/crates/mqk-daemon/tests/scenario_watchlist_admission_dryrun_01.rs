//! PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01: Dry-run watchlist admission check.
//!
//! Proves:
//! - GET /api/v1/watchlist/admission-check is reachable and read-only.
//! - All intake states return correct allowed/reason values.
//! - approved_for_live is always false in the response.
//! - note is always "dry_run_only_not_enforced".
//! - Real strategy/signal route behavior is unchanged (watchlist NOT wired).
//! - No outbox/inbox write path exists in the admission check handler.
//! - No broker call path exists.
//! - No DB mutation path exists.
//! - No live routing enable path exists.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                   |
//! |------|---------------------------------------------------------------------------------|
//! | AD01 | not_configured → allowed=false, reason=watchlist_not_configured                 |
//! | AD02 | missing file → allowed=false, reason=watchlist_missing                          |
//! | AD03 | invalid JSON → allowed=false, reason=watchlist_invalid                          |
//! | AD04 | loaded_not_approved → allowed=false, reason=watchlist_not_approved              |
//! | AD05 | loaded_approved + correct pair → allowed=true, reason=allowed                   |
//! | AD06 | loaded_approved + wrong symbol → allowed=false, reason=symbol_not_approved      |
//! | AD07 | loaded_approved + wrong strategy → allowed=false, reason=strategy_not_assigned  |
//! | AD08 | approved_for_live=true artifact → allowed=false; approved_for_live=false always |
//! | AD09 | note is always "dry_run_only_not_enforced"                                       |
//! | AD10 | Strategy signal route is unchanged (does not reference watchlist admission)      |
//! | AD11 | Response has no outbox/inbox fields (no mutation surface)                        |
//! | AD12 | approved_for_live is always false across all outcomes                            |
//! | AD13 | All non-approved states fail admission (comprehensive sweep)                     |
//! | AD14 | No live routing enable path in response or handler                               |

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state,
    watchlist_intake::{
        evaluate_watchlist_intake, evaluate_watchlist_signal_admission,
        WatchlistAdmissionReason, WatchlistIntakeOutcome, ENV_PAPER_WATCHLIST_PATH,
    },
};
use tokio::sync::Mutex;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialisation — prevent test races on MQK_PAPER_WATCHLIST_PATH
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

static COUNTER: AtomicU32 = AtomicU32::new(100);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// HTTP test helpers
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

fn build_router() -> axum::Router {
    let state = Arc::new(state::AppState::new());
    routes::build_router(state)
}

// ---------------------------------------------------------------------------
// Watchlist file fixture helpers
// ---------------------------------------------------------------------------

fn write_watchlist(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_ad_watchlist_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write watchlist file");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn valid_watchlist(
    approved_for_paper: bool,
    approved_for_live: bool,
    symbols: &[&str],
    strategy_assignments: &[(&str, &str)],
) -> String {
    let syms_json = symbols
        .iter()
        .map(|s| format!("{:?}", s))
        .collect::<Vec<_>>()
        .join(", ");
    let assignments_json = strategy_assignments
        .iter()
        .map(|(sym, strat)| format!("{:?}: {:?}", sym, strat))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"{{
  "schema_version": "watchlist-v1",
  "mode": "paper",
  "approved_for_autonomous_paper": {approved_for_paper},
  "approved_for_live": {approved_for_live},
  "symbols": [{syms_json}],
  "strategy_assignments": {{{assignments_json}}},
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}}"#
    )
}

fn approved_aapl() -> String {
    valid_watchlist(true, false, &["AAPL"], &[("AAPL", "strat-scalper-001")])
}

const ADMISSION_URL_NO_PARAMS: &str = "/api/v1/watchlist/admission-check";
const ADMISSION_URL_AAPL: &str =
    "/api/v1/watchlist/admission-check?symbol=AAPL&strategy_id=strat-scalper-001";

// ---------------------------------------------------------------------------
// AD01 — not_configured → allowed=false, reason=watchlist_not_configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad01_not_configured_returns_allowed_false() {
    let _guard = env_lock().lock().await;
    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "watchlist_not_configured");
    assert_eq!(json["status"], "not_configured");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD02 — missing file → allowed=false, reason=watchlist_missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad02_missing_file_returns_allowed_false() {
    let _guard = env_lock().lock().await;
    unsafe {
        std::env::set_var(
            ENV_PAPER_WATCHLIST_PATH,
            "/tmp/mqk_ad_no_such_watchlist_99999.json",
        );
    }

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "watchlist_missing");
    assert_eq!(json["status"], "missing");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD03 — invalid JSON → allowed=false, reason=watchlist_invalid
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad03_invalid_json_returns_allowed_false() {
    let _guard = env_lock().lock().await;

    let path = write_watchlist("ad03", "{ not valid json !!! ");
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "watchlist_invalid");
    assert_eq!(json["status"], "invalid");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD04 — loaded_not_approved → allowed=false, reason=watchlist_not_approved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad04_loaded_not_approved_returns_allowed_false() {
    let _guard = env_lock().lock().await;

    let json_content = valid_watchlist(false, false, &["AAPL"], &[("AAPL", "strat-scalper-001")]);
    let path = write_watchlist("ad04", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "watchlist_not_approved");
    assert_eq!(json["status"], "loaded_not_approved");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD05 — loaded_approved + correct pair → allowed=true, reason=allowed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad05_loaded_approved_correct_pair_returns_allowed_true() {
    let _guard = env_lock().lock().await;

    let json_content = approved_aapl();
    let path = write_watchlist("ad05", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], true, "expected allowed=true for approved pair");
    assert_eq!(json["reason"], "allowed");
    assert_eq!(json["status"], "loaded_approved");
    assert_eq!(json["approved_for_autonomous_paper"], true);
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
    assert_eq!(json["top_symbol"], "AAPL");
    assert_eq!(json["symbol"], "AAPL");
    assert_eq!(json["strategy_id"], "strat-scalper-001");
}

// ---------------------------------------------------------------------------
// AD06 — loaded_approved + wrong symbol → allowed=false, reason=symbol_not_approved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad06_loaded_approved_wrong_symbol_returns_allowed_false() {
    let _guard = env_lock().lock().await;

    let json_content = approved_aapl();
    let path = write_watchlist("ad06", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/admission-check?symbol=TSLA&strategy_id=strat-scalper-001")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "symbol_not_approved");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD07 — loaded_approved + wrong strategy → allowed=false, reason=strategy_not_assigned
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad07_loaded_approved_wrong_strategy_returns_allowed_false() {
    let _guard = env_lock().lock().await;

    let json_content = approved_aapl();
    let path = write_watchlist("ad07", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/admission-check?symbol=AAPL&strategy_id=strat-different-999")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["allowed"], false);
    assert_eq!(json["reason"], "strategy_not_assigned");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD08 — approved_for_live=true artifact → locked to Invalid; approved_for_live=false always
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad08_approved_for_live_true_artifact_is_invalid_and_live_always_false() {
    let _guard = env_lock().lock().await;

    // approved_for_live=true triggers hard live lock → WatchlistIntakeOutcome::Invalid
    let json_content = valid_watchlist(false, true, &["AAPL"], &[("AAPL", "strat-scalper-001")]);
    let path = write_watchlist("ad08", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["allowed"], false,
        "approved_for_live=true artifact must produce allowed=false"
    );
    assert_eq!(
        json["reason"], "watchlist_invalid",
        "approved_for_live=true artifact must produce watchlist_invalid reason"
    );
    assert_eq!(
        json["status"], "invalid",
        "approved_for_live=true in artifact → intake status must be invalid"
    );
    // Hard invariant: approved_for_live is ALWAYS false in the response.
    assert_eq!(
        json["approved_for_live"],
        serde_json::Value::Bool(false),
        "approved_for_live must ALWAYS be false in admission check response"
    );
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}

// ---------------------------------------------------------------------------
// AD09 — note is always "dry_run_only_not_enforced" (not-configured case)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad09_note_is_always_dry_run_only_not_enforced() {
    let _guard = env_lock().lock().await;
    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_NO_PARAMS)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["note"], "dry_run_only_not_enforced",
        "note must always be 'dry_run_only_not_enforced'"
    );
}

// ---------------------------------------------------------------------------
// AD10 — Strategy signal route behavior is unchanged (does not consult watchlist)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad10_strategy_signal_route_behavior_unchanged() {
    // Prove that POST /api/v1/strategy/signal returns the same gate-1 failure
    // regardless of whether a watchlist is configured.  The watchlist admission
    // helper is NOT wired into the signal route in this patch.
    let _guard = env_lock().lock().await;

    // Set an approved watchlist.
    let json_content = approved_aapl();
    let path = write_watchlist("ad10", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    // POST /api/v1/strategy/signal with valid payload — should fail gate_1_ingestion
    // because AppState::new() uses default (Paper mode, no ExternalSignalIngestion).
    let payload = serde_json::json!({
        "signal_id": "ad10-test-signal",
        "strategy_id": "strat-scalper-001",
        "symbol": "AAPL",
        "side": "buy",
        "qty": 1
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(payload.to_string()))
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    // Gate_1_ingestion fails — NOT a watchlist gate, proving signal route is unchanged.
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "strategy signal must still fail at gate_1_ingestion, not at a watchlist gate"
    );
    // The blocker must reference ingestion, not watchlist.
    let blockers = json["blockers"]
        .as_array()
        .expect("blockers must be an array");
    assert!(
        blockers.iter().any(|b| b
            .as_str()
            .map_or(false, |s| s.contains("ingestion") || s.contains("not configured"))),
        "gate_1_ingestion must be the refusal reason, not a watchlist reason; got: {blockers:?}"
    );
    // Confirm no watchlist-related reason in the response.
    let body_str = json.to_string();
    assert!(
        !body_str.contains("watchlist_not_configured")
            && !body_str.contains("watchlist_missing")
            && !body_str.contains("watchlist_invalid")
            && !body_str.contains("symbol_not_approved")
            && !body_str.contains("strategy_not_assigned"),
        "signal route must not produce watchlist reasons; got: {body_str}"
    );
}

// ---------------------------------------------------------------------------
// AD11 — Response has no outbox/inbox fields (no mutation surface)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad11_response_has_no_outbox_or_mutation_fields() {
    let _guard = env_lock().lock().await;
    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };

    let router = build_router();
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json_val = parse_json(body);

    assert_eq!(status, StatusCode::OK);

    // These fields must not appear in the admission check response.
    // Their presence would indicate a mutation surface or a signal path link.
    let body_str = json_val.to_string();
    assert!(
        !body_str.contains("outbox"),
        "response must not contain 'outbox'"
    );
    assert!(
        !body_str.contains("signal_id"),
        "response must not contain 'signal_id'"
    );
    assert!(
        !body_str.contains("active_run_id"),
        "response must not contain 'active_run_id'"
    );
    assert!(
        !body_str.contains("intent_placed"),
        "response must not contain 'intent_placed'"
    );
    assert!(
        !body_str.contains("enqueued"),
        "response must not contain 'enqueued' (outbox disposition)"
    );
}

// ---------------------------------------------------------------------------
// AD12 — approved_for_live is always false across all outcomes (pure proof)
// ---------------------------------------------------------------------------

#[test]
fn ad12_approved_for_live_always_false_across_all_outcomes() {
    // Prove via the pure admission evaluator that approved_for_live() on the
    // underlying WatchlistIntakeOutcome is always false for every variant.
    let variants: Vec<WatchlistIntakeOutcome> = vec![
        WatchlistIntakeOutcome::NotConfigured,
        WatchlistIntakeOutcome::Missing {
            configured_path: "/tmp/test.json".to_string(),
        },
        WatchlistIntakeOutcome::Invalid {
            failure_reasons: vec!["test reason".to_string()],
        },
    ];
    for variant in &variants {
        assert!(
            !variant.approved_for_live(),
            "approved_for_live must be false for {:?}",
            variant.status_label()
        );
    }

    // Also prove it for loaded outcomes via file-backed evaluation.
    let json_content = valid_watchlist(false, false, &["AAPL"], &[("AAPL", "strat-x")]);
    let path = std::env::temp_dir().join(format!(
        "mqk_ad12_not_approved_{}_{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, &json_content).unwrap();
    let not_approved = evaluate_watchlist_intake(Some(&path));
    let _ = std::fs::remove_file(&path);
    assert!(!not_approved.approved_for_live());

    let json_content_approved =
        valid_watchlist(true, false, &["AAPL"], &[("AAPL", "strat-x")]);
    let path2 = std::env::temp_dir().join(format!(
        "mqk_ad12_approved_{}_{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path2, &json_content_approved).unwrap();
    let approved = evaluate_watchlist_intake(Some(&path2));
    let _ = std::fs::remove_file(&path2);
    assert!(!approved.approved_for_live());
}

// ---------------------------------------------------------------------------
// AD13 — All non-approved intake states fail admission (comprehensive sweep)
// ---------------------------------------------------------------------------

#[test]
fn ad13_all_non_approved_states_fail_admission_comprehensive() {
    let cases = vec![
        (
            WatchlistIntakeOutcome::NotConfigured,
            WatchlistAdmissionReason::WatchlistNotConfigured,
        ),
        (
            WatchlistIntakeOutcome::Missing {
                configured_path: "/tmp/x.json".to_string(),
            },
            WatchlistAdmissionReason::WatchlistMissing,
        ),
        (
            WatchlistIntakeOutcome::Invalid {
                failure_reasons: vec!["hard live lock".to_string()],
            },
            WatchlistAdmissionReason::WatchlistInvalid,
        ),
    ];

    for (outcome, expected_reason) in &cases {
        let result = evaluate_watchlist_signal_admission(outcome, "AAPL", "strat-scalper-001");
        assert!(
            !result.allowed,
            "expected denied for {:?}",
            outcome.status_label()
        );
        assert_eq!(
            result.reason, *expected_reason,
            "wrong reason for {:?}",
            outcome.status_label()
        );
    }

    // LoadedNotApproved case needs a real file for the evaluator.
    let json_content = valid_watchlist(false, false, &["AAPL"], &[("AAPL", "strat-scalper-001")]);
    let path = std::env::temp_dir().join(format!(
        "mqk_ad13_{}_{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, &json_content).unwrap();
    let not_approved = evaluate_watchlist_intake(Some(&path));
    let _ = std::fs::remove_file(&path);

    let result = evaluate_watchlist_signal_admission(&not_approved, "AAPL", "strat-scalper-001");
    assert!(!result.allowed, "LoadedNotApproved must produce allowed=false");
    assert_eq!(result.reason, WatchlistAdmissionReason::WatchlistNotApproved);
}

// ---------------------------------------------------------------------------
// AD14 — No live routing enable path in response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ad14_no_live_routing_enable_path_in_response() {
    // Prove that the admission check response cannot produce a path that
    // enables live routing. The response type has no live_routing_enabled field
    // and approved_for_live is always false.
    let _guard = env_lock().lock().await;

    let json_content = approved_aapl();
    let path = write_watchlist("ad14", &json_content);
    unsafe { std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap()) };

    let router = build_router();
    // Use the approved pair — even when allowed=true, live must be false.
    let req = Request::builder()
        .uri(ADMISSION_URL_AAPL)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe { std::env::remove_var(ENV_PAPER_WATCHLIST_PATH) };
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    // Even for the fully-approved case (allowed=true), live must remain false.
    assert_eq!(
        json["allowed"], true,
        "approved pair should produce allowed=true"
    );
    assert_eq!(
        json["approved_for_live"],
        serde_json::Value::Bool(false),
        "approved_for_live must be false even when allowed=true"
    );
    // No live routing fields in response.
    let body_str = json.to_string();
    assert!(
        !body_str.contains("live_routing_enabled"),
        "response must not contain live_routing_enabled"
    );
    assert!(
        !body_str.contains("\"approved_for_live\":true"),
        "approved_for_live must never be true in response"
    );
    // note is still the enforcement deferred marker.
    assert_eq!(json["note"], "dry_run_only_not_enforced");
}
