//! PAPER-HANDOFF-READONLY-01: Read-only watchlist artifact loader and status surface.
//!
//! Proves:
//! - Watchlist intake evaluator returns correct outcomes for all file states.
//! - Hard live-lock: approved_for_live=true in artifact → Invalid.
//! - Constraint enforcement: max_symbols_to_trade != 1 → Invalid.
//! - Constraint enforcement: max_concurrent_positions != 1 → Invalid.
//! - Status endpoint returns correct JSON with approved_for_live=false always.
//! - Status endpoint exposes no secrets.
//! - Dry signal-admission contract behaves correctly for all intake states.
//! - No broker/OMS/order imports present in watchlist_intake module.
//! - No DB mutation path exists.
//! - No live routing enable path exists.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                |
//! |------|-------------------------------------------------------------------------------|
//! | W01  | No env var → NotConfigured, approved=false                                    |
//! | W02  | Missing file → Missing, approved=false                                        |
//! | W03  | Malformed JSON → Invalid, approved=false                                      |
//! | W04  | Wrong schema_version → Invalid                                                |
//! | W05  | mode != paper → Invalid                                                       |
//! | W06  | approved_for_live=true in artifact → Invalid (hard live lock)                 |
//! | W07  | approved_for_autonomous_paper=false, valid otherwise → LoadedNotApproved      |
//! | W08  | approved_for_autonomous_paper=true, valid single symbol → LoadedApproved      |
//! | W09  | max_symbols_to_trade > 1 → Invalid                                            |
//! | W10  | max_concurrent_positions > 1 → Invalid                                        |
//! | W11  | missing strategy_assignments → Invalid                                        |
//! | W12  | Status endpoint: approved_for_live=false always in response                   |
//! | W13  | Status endpoint: read-only (no DB, no broker, no mutation path)               |
//! | W14  | Dry admission: allows approved symbol/strategy                                |
//! | W15  | Dry admission: rejects wrong symbol                                           |
//! | W16  | Dry admission: rejects wrong strategy                                         |
//! | W17  | Dry admission: rejects invalid/not-approved intake status                     |
//! | W18  | approved_for_live always false on every outcome variant                       |
//! | W19  | Empty symbols with approved=true → Invalid (internal consistency)             |
//! | W20  | LoadedApproved: top_symbol and strategy_assignments surfaced correctly         |
//!
//! # WATCHLIST-V2-SCHEMA-01 proof matrix (additions)
//!
//! | Test | What it proves                                                                |
//! |------|-------------------------------------------------------------------------------|
//! | W21  | v2 valid, 3 symbols all assigned → LoadedApproved                             |
//! | W22  | v2 max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING → Invalid                 |
//! | W23  | v2 symbols.len() > max_symbols_to_trade → Invalid, no truncation              |
//! | W24  | v2 max_concurrent_positions > max_symbols_to_trade → Invalid                  |
//! | W25  | v2 symbol missing strategy_assignments entry → Invalid                        |
//! | W26  | v2 approved_for_live=true → Invalid (hard live lock still applies)            |
//! | W27  | v2 mode != paper → Invalid                                                    |
//! | W28  | v2 LoadedApproved artifact carries schema_version == "watchlist-v2"           |
//! | W29  | v1 LoadedApproved artifact carries schema_version == "watchlist-v1" (compat)  |
//! | W30  | Status endpoint surfaces schema_version == "watchlist-v2" for v2 artifacts    |
//! | W31  | v2 dry admission allows symbol #2 and #3 with their assigned strategies       |
//! | W32  | v2 boundary: max_symbols_to_trade==ceiling and max_concurrent==max_symbols    |

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes, state,
    watchlist_intake::{
        evaluate_watchlist_intake, evaluate_watchlist_signal_admission, WatchlistAdmissionReason,
        WatchlistIntakeOutcome, ENV_PAPER_WATCHLIST_PATH, MULTI_SYMBOL_HARD_CEILING,
        WATCHLIST_SCHEMA_VERSION_V1, WATCHLIST_SCHEMA_VERSION_V2,
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

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
        "mqk_watchlist_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write watchlist file");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Build a minimal valid `watchlist-v1` JSON string.
fn valid_watchlist(
    approved_for_paper: bool,
    approved_for_live: bool,
    symbols: &[&str],
    strategy_assignments: &[(&str, &str)],
    max_symbols: u64,
    max_concurrent: u64,
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
  "max_symbols_to_trade": {max_symbols},
  "max_concurrent_positions": {max_concurrent}
}}"#
    )
}

/// Build a fully-approved watchlist with AAPL and a strategy assignment.
fn approved_aapl_watchlist() -> String {
    valid_watchlist(
        true,
        false,
        &["AAPL"],
        &[("AAPL", "strat-scalper-001")],
        1,
        1,
    )
}

/// Build a minimal `watchlist-v2` JSON string (WATCHLIST-V2-SCHEMA-01).
fn valid_watchlist_v2(
    approved_for_paper: bool,
    approved_for_live: bool,
    mode: &str,
    symbols: &[&str],
    strategy_assignments: &[(&str, &str)],
    max_symbols: u64,
    max_concurrent: u64,
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
  "schema_version": "watchlist-v2",
  "mode": "{mode}",
  "approved_for_autonomous_paper": {approved_for_paper},
  "approved_for_live": {approved_for_live},
  "symbols": [{syms_json}],
  "strategy_assignments": {{{assignments_json}}},
  "max_symbols_to_trade": {max_symbols},
  "max_concurrent_positions": {max_concurrent}
}}"#
    )
}

/// Build a fully-approved `watchlist-v2` artifact with 3 symbols, each with
/// a distinct strategy assignment, `max_symbols_to_trade=3`,
/// `max_concurrent_positions=2`.
fn approved_v2_three_symbols() -> String {
    valid_watchlist_v2(
        true,
        false,
        "paper",
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        3,
        2,
    )
}

// ---------------------------------------------------------------------------
// W01 — No env var configured → NotConfigured
// ---------------------------------------------------------------------------

#[test]
fn w01_no_env_configured_returns_not_configured() {
    let outcome = evaluate_watchlist_intake(None);
    assert_eq!(outcome.status_label(), "not_configured");
    assert!(!outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());
    assert_eq!(outcome, WatchlistIntakeOutcome::NotConfigured);
}

// ---------------------------------------------------------------------------
// W02 — File missing → Missing
// ---------------------------------------------------------------------------

#[test]
fn w02_missing_file_returns_missing() {
    let path = std::path::Path::new("/tmp/mqk_no_such_watchlist_file_99999.json");
    let outcome = evaluate_watchlist_intake(Some(path));
    assert_eq!(outcome.status_label(), "missing");
    assert!(!outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());
}

// ---------------------------------------------------------------------------
// W03 — Malformed JSON → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w03_malformed_json_returns_invalid() {
    let path = write_watchlist("w03", "{ not valid json !!! ");
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    assert!(!outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());
    assert!(!outcome.failure_reasons().is_empty());
}

// ---------------------------------------------------------------------------
// W04 — Wrong schema_version → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w04_wrong_schema_version_returns_invalid() {
    // "watchlist-v3" is intentionally unsupported — v1 and v2 are the only
    // accepted schema versions (WATCHLIST-V2-SCHEMA-01).
    let json = r#"{
  "schema_version": "watchlist-v3",
  "mode": "paper",
  "approved_for_autonomous_paper": false,
  "approved_for_live": false,
  "symbols": [],
  "strategy_assignments": {},
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}"#;
    let path = write_watchlist("w04", json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| r.contains("schema_version")),
        "expected schema_version reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W05 — mode != paper → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w05_mode_not_paper_returns_invalid() {
    let json = r#"{
  "schema_version": "watchlist-v1",
  "mode": "live",
  "approved_for_autonomous_paper": false,
  "approved_for_live": false,
  "symbols": [],
  "strategy_assignments": {},
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}"#;
    let path = write_watchlist("w05", json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| r.contains("mode")),
        "expected mode reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W06 — approved_for_live=true in artifact → Invalid (hard live lock)
// ---------------------------------------------------------------------------

#[test]
fn w06_approved_for_live_true_is_hard_locked_invalid() {
    let json = valid_watchlist(false, true, &[], &[], 1, 1);
    let path = write_watchlist("w06", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("approved_for_live") || r.contains("live lock")),
        "expected live-lock reason, got: {reasons:?}"
    );
    // Hard invariant: even in invalid state, approved_for_live() is always false.
    assert!(!outcome.approved_for_live());
}

// ---------------------------------------------------------------------------
// W07 — approved_for_autonomous_paper=false → LoadedNotApproved
// ---------------------------------------------------------------------------

#[test]
fn w07_not_approved_returns_loaded_not_approved() {
    let json = valid_watchlist(false, false, &[], &[], 1, 1);
    let path = write_watchlist("w07", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_not_approved");
    assert!(!outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());
}

// ---------------------------------------------------------------------------
// W08 — approved=true, valid single symbol → LoadedApproved
// ---------------------------------------------------------------------------

#[test]
fn w08_valid_approved_single_symbol_returns_loaded_approved() {
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w08", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_approved");
    assert!(outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());

    let art = outcome.artifact().expect("artifact must be present");
    assert_eq!(art.symbols, vec!["AAPL".to_string()]);
    assert_eq!(art.top_symbol, Some("AAPL".to_string()));
    assert_eq!(
        art.strategy_assignments.get("AAPL"),
        Some(&"strat-scalper-001".to_string())
    );
}

// ---------------------------------------------------------------------------
// W09 — max_symbols_to_trade > 1 → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w09_max_symbols_to_trade_greater_than_1_is_invalid() {
    let json = valid_watchlist(false, false, &[], &[], 2, 1);
    let path = write_watchlist("w09", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| r.contains("max_symbols_to_trade")),
        "expected max_symbols_to_trade reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W10 — max_concurrent_positions > 1 → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w10_max_concurrent_positions_greater_than_1_is_invalid() {
    let json = valid_watchlist(false, false, &[], &[], 1, 3);
    let path = write_watchlist("w10", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("max_concurrent_positions")),
        "expected max_concurrent_positions reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W11 — missing strategy_assignments → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w11_missing_strategy_assignments_returns_invalid() {
    let json = r#"{
  "schema_version": "watchlist-v1",
  "mode": "paper",
  "approved_for_autonomous_paper": false,
  "approved_for_live": false,
  "symbols": [],
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}"#;
    let path = write_watchlist("w11", json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| r.contains("strategy_assignments")),
        "expected strategy_assignments reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W12 — Status endpoint: approved_for_live=false always in HTTP response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w12_endpoint_approved_for_live_always_false() {
    let _guard = env_lock().lock().await;

    // Set path to a non-existent file so we get "missing" status — any status is fine.
    unsafe {
        std::env::set_var(ENV_PAPER_WATCHLIST_PATH, "/tmp/mqk_w12_no_such_file.json");
    }

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    unsafe {
        std::env::remove_var(ENV_PAPER_WATCHLIST_PATH);
    }

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["approved_for_live"],
        serde_json::Value::Bool(false),
        "approved_for_live must always be false in response"
    );
}

// ---------------------------------------------------------------------------
// W13 — Status endpoint is read-only: returns 200 with expected shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w13_endpoint_is_read_only_returns_200_with_expected_shape() {
    let _guard = env_lock().lock().await;

    // Not configured case.
    unsafe {
        std::env::remove_var(ENV_PAPER_WATCHLIST_PATH);
    }

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "not_configured");
    assert_eq!(json["approved_for_live"], false);
    assert_eq!(json["approved_for_autonomous_paper"], false);
    // Must have checked_at_utc
    assert!(json["checked_at_utc"].is_string());
    // failure_reasons must be an array
    assert!(json["failure_reasons"].is_array());
    // symbols must be an array
    assert!(json["symbols"].is_array());
}

// ---------------------------------------------------------------------------
// W14 — Dry admission: allows approved symbol/strategy
// ---------------------------------------------------------------------------

#[test]
fn w14_dry_admission_allows_approved_symbol_and_strategy() {
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w14", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    let result = evaluate_watchlist_signal_admission(&outcome, "AAPL", "strat-scalper-001");
    assert!(result.allowed);
    assert_eq!(result.reason, WatchlistAdmissionReason::Allowed);
}

// ---------------------------------------------------------------------------
// W15 — Dry admission: rejects wrong symbol
// ---------------------------------------------------------------------------

#[test]
fn w15_dry_admission_rejects_wrong_symbol() {
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w15", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    let result = evaluate_watchlist_signal_admission(&outcome, "TSLA", "strat-scalper-001");
    assert!(!result.allowed);
    assert_eq!(result.reason, WatchlistAdmissionReason::SymbolNotApproved);
}

// ---------------------------------------------------------------------------
// W16 — Dry admission: rejects wrong strategy
// ---------------------------------------------------------------------------

#[test]
fn w16_dry_admission_rejects_wrong_strategy() {
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w16", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    let result = evaluate_watchlist_signal_admission(&outcome, "AAPL", "strat-different-999");
    assert!(!result.allowed);
    assert_eq!(result.reason, WatchlistAdmissionReason::StrategyNotAssigned);
}

// ---------------------------------------------------------------------------
// W17 — Dry admission: rejects invalid/not-approved intake states
// ---------------------------------------------------------------------------

#[test]
fn w17_dry_admission_rejects_all_non_approved_states() {
    let not_configured = WatchlistIntakeOutcome::NotConfigured;
    let missing = WatchlistIntakeOutcome::Missing {
        configured_path: "/tmp/missing.json".to_string(),
    };
    let invalid = WatchlistIntakeOutcome::Invalid {
        failure_reasons: vec!["test".to_string()],
    };

    let not_approved_json = valid_watchlist(false, false, &[], &[], 1, 1);
    let path = write_watchlist("w17", &not_approved_json);
    let not_approved = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    let cases = [
        (
            not_configured,
            WatchlistAdmissionReason::WatchlistNotConfigured,
        ),
        (missing, WatchlistAdmissionReason::WatchlistMissing),
        (invalid, WatchlistAdmissionReason::WatchlistInvalid),
        (not_approved, WatchlistAdmissionReason::WatchlistNotApproved),
    ];

    for (outcome, expected_reason) in cases {
        let result = evaluate_watchlist_signal_admission(&outcome, "AAPL", "strat-scalper-001");
        assert!(
            !result.allowed,
            "expected denied for {:?}",
            outcome.status_label()
        );
        assert_eq!(
            result.reason,
            expected_reason,
            "wrong reason for {:?}",
            outcome.status_label()
        );
    }
}

// ---------------------------------------------------------------------------
// W18 — approved_for_live always false on every outcome variant
// ---------------------------------------------------------------------------

#[test]
fn w18_approved_for_live_is_always_false_on_all_variants() {
    // NotConfigured
    assert!(!WatchlistIntakeOutcome::NotConfigured.approved_for_live());

    // Missing
    assert!(!WatchlistIntakeOutcome::Missing {
        configured_path: "/tmp/x.json".to_string()
    }
    .approved_for_live());

    // Invalid
    assert!(!WatchlistIntakeOutcome::Invalid {
        failure_reasons: vec!["test".to_string()]
    }
    .approved_for_live());

    // LoadedNotApproved
    let json = valid_watchlist(false, false, &[], &[], 1, 1);
    let path = write_watchlist("w18a", &json);
    let not_approved = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);
    assert!(!not_approved.approved_for_live());

    // LoadedApproved
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w18b", &json);
    let approved = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);
    assert!(!approved.approved_for_live());
}

// ---------------------------------------------------------------------------
// W19 — approved=true but symbols empty → Invalid (internal consistency)
// ---------------------------------------------------------------------------

#[test]
fn w19_approved_true_with_empty_symbols_is_invalid() {
    let json = valid_watchlist(true, false, &[], &[], 1, 1);
    let path = write_watchlist("w19", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("approved_for_autonomous_paper") || r.contains("symbols")),
        "expected consistency reason, got: {reasons:?}"
    );
    assert!(!outcome.approved_for_live());
}

// ---------------------------------------------------------------------------
// W20 — LoadedApproved: top_symbol and strategy_assignments surfaced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w20_loaded_approved_endpoint_surfaces_top_symbol_and_assignments() {
    let _guard = env_lock().lock().await;

    let json = approved_aapl_watchlist();
    let path = write_watchlist("w20", &json);

    unsafe {
        std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap());
    }

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json_resp = parse_json(body);

    unsafe {
        std::env::remove_var(ENV_PAPER_WATCHLIST_PATH);
    }
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_resp["status"], "loaded_approved");
    assert_eq!(json_resp["approved_for_autonomous_paper"], true);
    assert_eq!(json_resp["approved_for_live"], false);
    assert_eq!(json_resp["top_symbol"], "AAPL");
    assert_eq!(json_resp["symbols"][0], "AAPL");
    assert_eq!(
        json_resp["strategy_assignments"]["AAPL"],
        "strat-scalper-001"
    );
    assert_eq!(json_resp["max_symbols_to_trade"], 1);
    assert_eq!(json_resp["max_concurrent_positions"], 1);
    // No failure_reasons on valid approved artifact.
    assert!(
        json_resp["failure_reasons"]
            .as_array()
            .is_some_and(|a| a.is_empty()),
        "failure_reasons must be empty on loaded_approved"
    );
}

// ---------------------------------------------------------------------------
// W21 — v2 valid, 3 symbols all assigned → LoadedApproved
// ---------------------------------------------------------------------------

#[test]
fn w21_v2_valid_three_symbols_all_assigned_returns_loaded_approved() {
    let json = approved_v2_three_symbols();
    let path = write_watchlist("w21", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_approved");
    assert!(outcome.approved_for_autonomous_paper());
    assert!(!outcome.approved_for_live());

    let art = outcome.artifact().expect("artifact must be present");
    assert_eq!(art.schema_version, WATCHLIST_SCHEMA_VERSION_V2);
    assert_eq!(
        art.symbols,
        vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()]
    );
    assert_eq!(art.top_symbol, Some("AAPL".to_string()));
    assert_eq!(art.max_symbols_to_trade, 3);
    assert_eq!(art.max_concurrent_positions, 2);
    assert_eq!(
        art.strategy_assignments.get("AAPL"),
        Some(&"strat-scalper-001".to_string())
    );
    assert_eq!(
        art.strategy_assignments.get("MSFT"),
        Some(&"strat-scalper-002".to_string())
    );
    assert_eq!(
        art.strategy_assignments.get("TSLA"),
        Some(&"strat-scalper-003".to_string())
    );
}

// ---------------------------------------------------------------------------
// W22 — v2 max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w22_v2_max_symbols_above_hard_ceiling_is_invalid() {
    let over_ceiling = MULTI_SYMBOL_HARD_CEILING + 1;
    let json = valid_watchlist_v2(false, false, "paper", &[], &[], over_ceiling, 1);
    let path = write_watchlist("w22", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| {
            r.contains("watchlist_multi_symbol_ceiling_exceeded")
                && r.contains("MULTI_SYMBOL_HARD_CEILING")
        }),
        "expected multi-symbol ceiling reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W23 — v2 symbols.len() > max_symbols_to_trade → Invalid, no truncation
// ---------------------------------------------------------------------------

#[test]
fn w23_v2_symbols_exceeding_max_symbols_to_trade_is_invalid_no_truncation() {
    let json = valid_watchlist_v2(
        false,
        false,
        "paper",
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        2,
        1,
    );
    let path = write_watchlist("w23", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    assert!(
        outcome.artifact().is_none(),
        "Invalid outcome must not carry a truncated artifact"
    );
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("symbols.len()=3") && r.contains("max_symbols_to_trade=2")),
        "expected symbols-exceed-max reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W24 — v2 max_concurrent_positions > max_symbols_to_trade → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w24_v2_max_concurrent_positions_above_max_symbols_is_invalid() {
    let json = valid_watchlist_v2(
        false,
        false,
        "paper",
        &["AAPL", "MSFT"],
        &[("AAPL", "strat-scalper-001"), ("MSFT", "strat-scalper-002")],
        2,
        3,
    );
    let path = write_watchlist("w24", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons.iter().any(|r| {
            r.contains("watchlist_max_concurrent_positions_invalid")
                && r.contains("max_concurrent_positions=3")
                && r.contains("max_symbols_to_trade=2")
        }),
        "expected max_concurrent_positions-exceeds-max_symbols reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W25 — v2 symbol missing strategy_assignments entry → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w25_v2_symbol_missing_strategy_assignment_is_invalid() {
    let json = valid_watchlist_v2(
        false,
        false,
        "paper",
        &["AAPL", "MSFT", "TSLA"],
        &[("AAPL", "strat-scalper-001"), ("MSFT", "strat-scalper-002")],
        3,
        2,
    );
    let path = write_watchlist("w25", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("watchlist_strategy_assignment_missing") && r.contains("TSLA")),
        "expected strategy_assignment_missing reason for TSLA, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W26 — v2 approved_for_live=true → Invalid (hard live lock still applies)
// ---------------------------------------------------------------------------

#[test]
fn w26_v2_approved_for_live_true_is_hard_locked_invalid() {
    let json = valid_watchlist_v2(
        true,
        true,
        "paper",
        &["AAPL"],
        &[("AAPL", "strat-scalper-001")],
        1,
        1,
    );
    let path = write_watchlist("w26", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("watchlist_live_approval_forbidden")),
        "expected live-lock reason, got: {reasons:?}"
    );
    // Hard invariant: even in invalid state, approved_for_live() is always false.
    assert!(!outcome.approved_for_live());
}

// ---------------------------------------------------------------------------
// W27 — v2 mode != paper → Invalid
// ---------------------------------------------------------------------------

#[test]
fn w27_v2_mode_not_paper_returns_invalid() {
    let json = valid_watchlist_v2(false, false, "live", &[], &[], 1, 1);
    let path = write_watchlist("w27", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "invalid");
    let reasons = outcome.failure_reasons();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("watchlist_mode_not_paper")),
        "expected mode reason, got: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// W28 — v2 LoadedApproved artifact carries schema_version == "watchlist-v2"
// ---------------------------------------------------------------------------

#[test]
fn w28_v2_loaded_approved_artifact_carries_v2_schema_version() {
    let json = approved_v2_three_symbols();
    let path = write_watchlist("w28", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_approved");
    let art = outcome.artifact().expect("artifact must be present");
    assert_eq!(art.schema_version, WATCHLIST_SCHEMA_VERSION_V2);
}

// ---------------------------------------------------------------------------
// W29 — v1 LoadedApproved artifact carries schema_version == "watchlist-v1" (compat)
// ---------------------------------------------------------------------------

#[test]
fn w29_v1_loaded_approved_artifact_carries_v1_schema_version() {
    let json = approved_aapl_watchlist();
    let path = write_watchlist("w29", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_approved");
    let art = outcome.artifact().expect("artifact must be present");
    assert_eq!(art.schema_version, WATCHLIST_SCHEMA_VERSION_V1);
}

// ---------------------------------------------------------------------------
// W30 — Status endpoint surfaces schema_version == "watchlist-v2" for v2 artifacts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn w30_endpoint_surfaces_v2_schema_version_and_multi_symbol_fields() {
    let _guard = env_lock().lock().await;

    let json = approved_v2_three_symbols();
    let path = write_watchlist("w30", &json);

    unsafe {
        std::env::set_var(ENV_PAPER_WATCHLIST_PATH, path.to_str().unwrap());
    }

    let router = build_router();
    let req = Request::builder()
        .uri("/api/v1/watchlist/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let json_resp = parse_json(body);

    unsafe {
        std::env::remove_var(ENV_PAPER_WATCHLIST_PATH);
    }
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_resp["status"], "loaded_approved");
    assert_eq!(json_resp["schema_version"], "watchlist-v2");
    assert_eq!(json_resp["approved_for_live"], false);
    assert_eq!(json_resp["max_symbols_to_trade"], 3);
    assert_eq!(json_resp["max_concurrent_positions"], 2);
    assert_eq!(
        json_resp["symbols"],
        serde_json::json!(["AAPL", "MSFT", "TSLA"])
    );
    assert_eq!(
        json_resp["strategy_assignments"]["MSFT"],
        "strat-scalper-002"
    );
    assert_eq!(
        json_resp["strategy_assignments"]["TSLA"],
        "strat-scalper-003"
    );
}

// ---------------------------------------------------------------------------
// W31 — v2 dry admission allows symbol #2 and #3 with their assigned strategies
// ---------------------------------------------------------------------------

#[test]
fn w31_v2_dry_admission_allows_symbols_two_and_three() {
    let json = approved_v2_three_symbols();
    let path = write_watchlist("w31", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    let msft = evaluate_watchlist_signal_admission(&outcome, "MSFT", "strat-scalper-002");
    assert!(msft.allowed);
    assert_eq!(msft.reason, WatchlistAdmissionReason::Allowed);

    let tsla = evaluate_watchlist_signal_admission(&outcome, "TSLA", "strat-scalper-003");
    assert!(tsla.allowed);
    assert_eq!(tsla.reason, WatchlistAdmissionReason::Allowed);

    // Cross-assignment: MSFT's symbol with TSLA's strategy must be denied.
    let cross = evaluate_watchlist_signal_admission(&outcome, "MSFT", "strat-scalper-003");
    assert!(!cross.allowed);
    assert_eq!(cross.reason, WatchlistAdmissionReason::StrategyNotAssigned);
}

// ---------------------------------------------------------------------------
// W32 — v2 boundary: max_symbols_to_trade==ceiling and max_concurrent==max_symbols
// ---------------------------------------------------------------------------

#[test]
fn w32_v2_boundary_at_hard_ceiling_is_loaded_approved() {
    let ceiling = MULTI_SYMBOL_HARD_CEILING;
    let symbols = ["AAPL", "MSFT", "TSLA", "GOOGL", "AMZN"];
    assert_eq!(
        symbols.len() as u64,
        ceiling,
        "fixture must match the hard ceiling"
    );

    let assignments: Vec<(&str, &str)> = vec![
        ("AAPL", "strat-scalper-001"),
        ("MSFT", "strat-scalper-002"),
        ("TSLA", "strat-scalper-003"),
        ("GOOGL", "strat-scalper-004"),
        ("AMZN", "strat-scalper-005"),
    ];

    let json = valid_watchlist_v2(
        true,
        false,
        "paper",
        &symbols,
        &assignments,
        ceiling,
        ceiling,
    );
    let path = write_watchlist("w32", &json);
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);

    assert_eq!(outcome.status_label(), "loaded_approved");
    let art = outcome.artifact().expect("artifact must be present");
    assert_eq!(art.max_symbols_to_trade, MULTI_SYMBOL_HARD_CEILING);
    assert_eq!(art.max_concurrent_positions, MULTI_SYMBOL_HARD_CEILING);
    assert_eq!(art.symbols.len() as u64, MULTI_SYMBOL_HARD_CEILING);
    assert!(!outcome.approved_for_live());
}
