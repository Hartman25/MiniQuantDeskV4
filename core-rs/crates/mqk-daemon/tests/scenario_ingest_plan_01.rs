//! WATCHLIST-INGEST-PLAN-01 — Read-only required-symbol ingest plan proof
//!
//! ## Purpose
//!
//! `GET /api/v1/market-data/ingest-plan` answers, in one place: which
//! symbols/timeframe does the bot require for trading readiness, where did
//! that list come from, and which source should premarket ingest and the
//! GUI coverage panel expect. It reuses
//! `market_data_freshness::required_symbols_with_source_from_env` — the
//! exact resolver `PREMARKET-DATA-READINESS-GATE-01` already uses for the
//! startup readiness gate — so this surface and that gate can never
//! disagree about which symbols are required.
//!
//! These tests exercise the route exclusively over HTTP (the response
//! builder is a private route-module helper, consistent with
//! `routes/watchlist.rs::build_watchlist_status_response` not being
//! unit-tested directly either). No DB, provider, or broker access is ever
//! exercised — the handler takes no `State<AppState>` at all.
//!
//! | Test    | Claim                                                                                   |
//! |---------|------------------------------------------------------------------------------------------|
//! | IP-01   | legacy MQK_STRATEGY_SYMBOL fallback -> one symbol, source=env_strategy_symbol, active     |
//! | IP-02   | approved watchlist-v2 wins over legacy env fallback -> source=watchlist_v2, active        |
//! | IP-03   | watchlist path configured but file missing -> degraded, falls back to legacy symbol      |
//! | IP-04   | watchlist path configured but invalid JSON -> degraded, falls back to legacy symbol      |
//! | IP-05   | watchlist missing AND no legacy fallback -> degraded, source=none, empty symbols          |
//! | IP-06   | nothing configured -> not_configured, source=none, empty symbols (no registry fallback)  |
//! | IP-07   | timeframe absent even though legacy symbol set -> not_configured, empty symbols           |
//! | IP-08   | watchlist-v2 symbols normalize whitespace/case duplicates end-to-end through the route    |
//! | IP-09   | response shape: every field present with the documented JSON type                        |
//! | IP-10   | route requires no DB/provider/broker state and is mounted on the public (no-auth) router |
//! | IP-11   | watchlist loaded but approved_for_autonomous_paper=false -> degraded, falls back        |
//! | IP-12   | ingest-plan and system/preflight market_data_readiness agree on required symbols/tf      |

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Env-var serialization — tests that mutate process-global env vars
// (MQK_STRATEGY_SYMBOL / MQK_STRATEGY_MD_TIMEFRAME / MQK_PAPER_WATCHLIST_PATH)
// must not run concurrently with each other. Mirrors
// scenario_premarket_data_readiness_gate_01.rs.
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_ingest_plan_env() {
    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }
}

// ---------------------------------------------------------------------------
// Watchlist-v2 fixture helpers (mirrors scenario_premarket_data_readiness_gate_01.rs)
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn write_watchlist(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_ip_watchlist_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write watchlist fixture file");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn valid_watchlist_v2_raw(
    symbols_json: &str,
    assignments_json: &str,
    max_symbols: u64,
    max_concurrent: u64,
) -> String {
    format!(
        r#"{{
  "schema_version": "watchlist-v2",
  "mode": "paper",
  "approved_for_autonomous_paper": true,
  "approved_for_live": false,
  "symbols": [{symbols_json}],
  "strategy_assignments": {{{assignments_json}}},
  "max_symbols_to_trade": {max_symbols},
  "max_concurrent_positions": {max_concurrent}
}}"#
    )
}

fn valid_watchlist_v2(
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
    valid_watchlist_v2_raw(&syms_json, &assignments_json, max_symbols, max_concurrent)
}

// ---------------------------------------------------------------------------
// HTTP test helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    use tower::ServiceExt;
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

async fn get_ingest_plan() -> serde_json::Value {
    let st = std::sync::Arc::new(state::AppState::new());
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        Request::builder()
            .uri("/api/v1/market-data/ingest-plan")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ingest-plan route must always return 200; body={:?}",
        String::from_utf8_lossy(&body)
    );
    parse_json(body)
}

fn sorted_strings(v: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .expect("expected JSON array")
        .iter()
        .map(|e| e.as_str().expect("expected string entry").to_string())
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// IP-01 — legacy single MQK_STRATEGY_SYMBOL fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip01_legacy_single_symbol_fallback_returns_one_symbol_and_source() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["symbol_source"], "env_strategy_symbol");
    assert_eq!(v["required_symbols"], serde_json::json!(["AAPL"]));
    assert_eq!(v["timeframe"], "1D");
    assert_eq!(v["required_symbol_timeframes"].as_array().unwrap().len(), 1);
    assert_eq!(v["required_symbol_timeframes"][0]["symbol"], "AAPL");
    assert_eq!(v["required_symbol_timeframes"][0]["timeframe"], "1D");
    assert_eq!(
        v["required_symbol_timeframes"][0]["source"],
        "env_strategy_symbol"
    );
    assert!(
        v["warnings"].as_array().unwrap().is_empty(),
        "IP-01: unambiguous legacy resolution must carry no warnings; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-02 — approved watchlist-v2 wins over legacy env fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip02_approved_watchlist_v2_wins_over_legacy_symbol() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let json = valid_watchlist_v2(
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        3,
        2,
    );
    let path = write_watchlist("ip02", &json);
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_SYMBOL", "SPY");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();
    cleanup(&path);

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["symbol_source"], "watchlist_v2");
    assert_eq!(
        sorted_strings(&v["required_symbols"]),
        vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()]
    );
    assert_eq!(v["timeframe"], "1D");
    assert!(
        !v["required_symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "SPY"),
        "IP-02: watchlist-v2 must supersede the legacy single symbol; got {v}"
    );
    assert!(
        v["warnings"].as_array().unwrap().is_empty(),
        "IP-02: an active, usable watchlist must carry no warnings; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-03 — watchlist path configured but file missing -> degraded + fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip03_watchlist_missing_falls_back_to_legacy_symbol_and_is_degraded() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let missing_path = std::env::temp_dir().join(format!(
        "mqk_ip03_does_not_exist_{}_{}",
        std::process::id(),
        next_id()
    ));
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", missing_path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert_eq!(
        v["truth_state"], "degraded",
        "IP-03: configured-but-missing watchlist must surface as degraded; got {v}"
    );
    assert_eq!(v["symbol_source"], "env_strategy_symbol");
    assert_eq!(v["required_symbols"], serde_json::json!(["AAPL"]));
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            let s = w.as_str().unwrap_or_default();
            s.contains("MQK_PAPER_WATCHLIST_PATH") && s.contains("missing")
        }),
        "IP-03: warnings must explain the missing watchlist; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-04 — watchlist path configured but invalid JSON -> degraded + fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip04_watchlist_invalid_falls_back_to_legacy_symbol_and_is_degraded() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let path = write_watchlist("ip04", "{ not valid json ");
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();
    cleanup(&path);

    assert_eq!(
        v["truth_state"], "degraded",
        "IP-04: configured-but-invalid watchlist must surface as degraded; got {v}"
    );
    assert_eq!(v["symbol_source"], "env_strategy_symbol");
    assert_eq!(v["required_symbols"], serde_json::json!(["AAPL"]));
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            let s = w.as_str().unwrap_or_default();
            s.contains("MQK_PAPER_WATCHLIST_PATH") && s.contains("invalid")
        }),
        "IP-04: warnings must explain the invalid watchlist; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-05 — watchlist missing AND no legacy fallback -> degraded, empty symbols
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip05_watchlist_missing_with_no_fallback_is_degraded_with_empty_symbols() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let missing_path = std::env::temp_dir().join(format!(
        "mqk_ip05_does_not_exist_{}_{}",
        std::process::id(),
        next_id()
    ));
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", missing_path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
        // Deliberately no MQK_STRATEGY_SYMBOL — no fallback is available.
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert_eq!(
        v["truth_state"], "degraded",
        "IP-05: a configured-but-broken watchlist is degraded even with no fallback; got {v}"
    );
    assert_eq!(v["symbol_source"], "none");
    assert_eq!(v["required_symbols"], serde_json::json!([]));
    assert_eq!(v["required_symbol_timeframes"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------
// IP-06 — nothing configured -> not_configured, no instrument-registry fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip06_nothing_configured_is_not_configured_and_never_uses_instrument_registry() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert_eq!(v["truth_state"], "not_configured");
    assert_eq!(v["symbol_source"], "none");
    assert_eq!(v["required_symbols"], serde_json::json!([]));
    assert!(v["timeframe"].is_null());
    // PRIMARY GOAL: the ~80-symbol instrument registry must never become the
    // default trading watchlist. required_symbols must be empty, not a
    // large registry-derived universe.
    assert_eq!(
        v["required_symbols"].as_array().unwrap().len(),
        0,
        "IP-06: ingest plan must not fall back to the instrument registry; got {v}"
    );
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .unwrap_or_default()
            .contains("MQK_STRATEGY_MD_TIMEFRAME")),
        "IP-06: warnings must explain the missing timeframe; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-07 — timeframe absent even though legacy symbol is set -> not_configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip07_timeframe_absent_is_not_configured_even_if_symbol_set() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        // Deliberately no MQK_STRATEGY_MD_TIMEFRAME.
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert_eq!(
        v["truth_state"], "not_configured",
        "IP-07: absent timeframe must block resolution regardless of symbol; got {v}"
    );
    assert_eq!(v["symbol_source"], "none");
    assert_eq!(v["required_symbols"], serde_json::json!([]));
    assert!(v["timeframe"].is_null());
}

// ---------------------------------------------------------------------------
// IP-08 — watchlist-v2 symbols normalize whitespace/case duplicates end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip08_watchlist_v2_symbols_normalize_whitespace_and_case_through_route() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    // "aapl" and " AAPL " are distinct raw strings (no validation-level
    // dedup), each carrying its own strategy_assignments entry — but the
    // ingest-plan route must still normalize them down to one AAPL via
    // normalize_required_symbols, exactly like the readiness gate does.
    let json = valid_watchlist_v2_raw(
        r#""aapl", " AAPL ", "MSFT""#,
        r#""aapl": "strat-a", " AAPL ": "strat-b", "MSFT": "strat-c""#,
        3,
        2,
    );
    let path = write_watchlist("ip08", &json);
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();
    cleanup(&path);

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["symbol_source"], "watchlist_v2");
    assert_eq!(
        sorted_strings(&v["required_symbols"]),
        vec!["AAPL".to_string(), "MSFT".to_string()],
        "IP-08: whitespace/case duplicates of AAPL must collapse to one normalized entry; got {v}"
    );
    assert_eq!(
        v["required_symbol_timeframes"].as_array().unwrap().len(),
        2,
        "IP-08: normalized list must drive required_symbol_timeframes too; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-09 — response shape: every field present with the documented JSON type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip09_response_shape_has_all_documented_fields_with_correct_types() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert!(v["canonical_route"].is_string());
    assert_eq!(v["canonical_route"], "/api/v1/market-data/ingest-plan");
    assert!(v["truth_state"].is_string());
    assert!(v["symbol_source"].is_string());
    assert!(v["required_symbols"].is_array());
    assert!(
        v["timeframe"].is_string(),
        "timeframe must be present (Some) in this fixture"
    );
    assert!(v["required_symbol_timeframes"].is_array());
    for entry in v["required_symbol_timeframes"].as_array().unwrap() {
        assert!(entry["symbol"].is_string());
        assert!(entry["timeframe"].is_string());
        assert!(entry["source"].is_string());
    }
    assert!(v["coverage_expectation"].is_object());
    assert_eq!(
        v["coverage_expectation"]["uses_market_data_readiness"],
        true
    );
    assert_eq!(v["coverage_expectation"]["uses_md_bars"], true);
    assert!(v["warnings"].is_array());
    assert!(v["checked_at_utc"].is_string());
    // RFC3339 sanity check — must parse as a timestamp.
    assert!(
        chrono::DateTime::parse_from_rfc3339(v["checked_at_utc"].as_str().unwrap()).is_ok(),
        "IP-09: checked_at_utc must be RFC3339; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-10 — route requires no DB/provider/broker state; mounted on public router
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip10_route_requires_no_db_and_is_publicly_mounted_without_auth() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();

    // AppState::new() has no DB pool and no broker config configured by
    // this test. A 200 response here proves the route does not require
    // DB/provider/broker state to function (it takes no State<AppState> at
    // all) and is reachable without authentication (no Authorization header
    // sent).
    let v = get_ingest_plan().await;
    clear_ingest_plan_env();

    assert!(v["truth_state"].is_string());
}

// ---------------------------------------------------------------------------
// IP-11 — watchlist loaded but not approved -> degraded, falls back to legacy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip11_watchlist_not_approved_falls_back_and_is_degraded() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let json = r#"{
  "schema_version": "watchlist-v2",
  "mode": "paper",
  "approved_for_autonomous_paper": false,
  "approved_for_live": false,
  "symbols": ["AAPL", "MSFT"],
  "strategy_assignments": {"AAPL": "strat-a", "MSFT": "strat-b"},
  "max_symbols_to_trade": 2,
  "max_concurrent_positions": 1
}"#
    .to_string();
    let path = write_watchlist("ip11", &json);
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_SYMBOL", "SPY");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let v = get_ingest_plan().await;
    clear_ingest_plan_env();
    cleanup(&path);

    assert_eq!(
        v["truth_state"], "degraded",
        "IP-11: a loaded-but-not-approved watchlist must surface as degraded; got {v}"
    );
    assert_eq!(v["symbol_source"], "env_strategy_symbol");
    assert_eq!(v["required_symbols"], serde_json::json!(["SPY"]));
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            let s = w.as_str().unwrap_or_default();
            s.contains("MQK_PAPER_WATCHLIST_PATH") && s.contains("loaded_not_approved")
        }),
        "IP-11: warnings must explain the not-approved watchlist; got {v}"
    );
}

// ---------------------------------------------------------------------------
// IP-12 — cross-surface proof: ingest-plan and system/preflight's
// market_data_readiness agree on required symbols/timeframe
// (PREMARKET-INGEST-PLAN-PROOF-01)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ip12_ingest_plan_and_preflight_market_data_readiness_agree_on_required_symbols() {
    let _g = env_lock().lock().await;
    clear_ingest_plan_env();
    let json = valid_watchlist_v2(
        &["AAPL", "MSFT"],
        &[("AAPL", "strat-a"), ("MSFT", "strat-b")],
        2,
        2,
    );
    let path = write_watchlist("ip12", &json);
    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        // Decoy legacy symbol: must not leak into either surface once a
        // watchlist-v2 artifact is approved and active.
        std::env::set_var("MQK_STRATEGY_SYMBOL", "SPY");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let plan = get_ingest_plan().await;

    // system/preflight only populates market_data_readiness for paper+alpaca
    // (PREMARKET-DATA-READINESS-GATE-01's is_paper_alpaca gate); mirrors
    // scenario_premarket_data_readiness_gate_01.rs::make_paper_alpaca().
    let st = std::sync::Arc::new(state::AppState::new_for_test_with_broker_kind(
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    clear_ingest_plan_env();
    cleanup(&path);

    assert_eq!(status, StatusCode::OK);
    let preflight = parse_json(body);
    let readiness = &preflight["market_data_readiness"];
    assert!(
        !readiness.is_null(),
        "IP-12: paper+alpaca preflight must return a market_data_readiness object; got {preflight}"
    );

    assert_eq!(
        plan["symbol_source"], "watchlist_v2",
        "IP-12: ingest-plan must resolve the watchlist-v2 source; got {plan}"
    );
    assert_eq!(
        sorted_strings(&plan["required_symbols"]),
        sorted_strings(&readiness["required_symbols"]),
        "IP-12: ingest-plan and system/preflight market_data_readiness must agree on \
         required_symbols -- both reuse required_symbols_with_source_from_env; \
         ingest_plan={plan} preflight_readiness={readiness}"
    );
    assert!(
        !sorted_strings(&plan["required_symbols"])
            .iter()
            .any(|s| s == "SPY"),
        "IP-12: the decoy legacy symbol must not leak into either surface once watchlist-v2 \
         is active; got ingest_plan={plan}"
    );

    // Per-symbol rows on the preflight side must carry the exact timeframe
    // ingest-plan resolved, proving the two surfaces share not just the
    // symbol set but also the timeframe.
    let plan_timeframe = plan["timeframe"].as_str().unwrap();
    let per_symbol = readiness["per_symbol"].as_array().unwrap();
    assert_eq!(
        per_symbol.len(),
        2,
        "IP-12: preflight must evaluate both watchlist-v2 symbols; got {readiness}"
    );
    for row in per_symbol {
        assert_eq!(
            row["timeframe"].as_str().unwrap(),
            plan_timeframe,
            "IP-12: preflight per-symbol timeframe must match ingest-plan timeframe; \
             row={row} plan_timeframe={plan_timeframe}"
        );
    }
}
