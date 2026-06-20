//! PREMARKET-DATA-READINESS-GATE-01 — Multi-symbol premarket readiness gate proof
//!
//! ## Purpose
//!
//! Extends `DATA-FRESHNESS-READINESS-GATE-01` (single symbol/timeframe) to a
//! multi-symbol premarket readiness gate: every symbol the current
//! deployment requires must have sufficient fresh completed bars in
//! `md_bars` before `start_execution_runtime` allows a paper/autonomous run
//! to start. Any single required symbol that is missing, insufficient, or
//! stale blocks start for the whole run (fail-closed).
//!
//! These tests exercise pure functions
//! (`normalize_required_symbols`, `aggregate_freshness_statuses`,
//! `required_symbols_for_freshness_gate_from_env`), the async DB-optional
//! glue (`evaluate_md_freshness_status_for_symbols`), and the additive API
//! surface (`market_data_readiness` on `system/preflight` and
//! `autonomous/readiness`). None call or reference broker, order, or outbox
//! code — they only read `md_bars` (via the pre-existing single-symbol
//! evaluator) and the watchlist-intake artifact reader.
//!
//! | Test      | Claim                                                                                  |
//! |-----------|-----------------------------------------------------------------------------------------|
//! | PMR-N01   | normalize_required_symbols trims/uppercases and dedupes whitespace/case duplicates      |
//! | PMR-N02   | normalize_required_symbols drops entries with an empty symbol or timeframe              |
//! | PMR-N03   | normalize_required_symbols keeps distinct timeframes for the same symbol distinct       |
//! | PMR-A01   | aggregate: empty per_symbol -> not_applicable, start_allowed=true                       |
//! | PMR-A02   | aggregate: single symbol "ok" -> aggregate "ok" (backward compat, n=1)                  |
//! | PMR-A03   | aggregate: single symbol "missing" -> aggregate "missing" (backward compat, n=1)        |
//! | PMR-A04   | aggregate: multi-symbol all ok -> aggregate "ok", start_allowed=true                    |
//! | PMR-A05   | aggregate: multi-symbol one missing -> blocked, blocker names the symbol                |
//! | PMR-A06   | aggregate: multi-symbol one stale -> blocked, blocker names the symbol                  |
//! | PMR-A07   | aggregate: multi-symbol one insufficient -> blocked, blocker names the symbol           |
//! | PMR-A08   | aggregate: multi-symbol two blocking symbols -> "mixed_blocked", both named             |
//! | PMR-A09   | aggregate: multi-symbol all unavailable, no blockers -> "unavailable", start allowed    |
//! | PMR-E01   | evaluate_for_symbols, db=None, single symbol -> "unavailable", not a blocker            |
//! | PMR-E02   | evaluate_for_symbols, empty required -> "not_applicable"                                |
//! | PMR-E03   | evaluate_for_symbols dedupes whitespace/case duplicates before evaluating               |
//! | PMR-E04   | evaluate_for_symbols (n=1) is byte-identical to the legacy single-symbol evaluator      |
//! | PMR-R01   | resolver: legacy MQK_STRATEGY_SYMBOL used when no watchlist; independent of IDS         |
//! | PMR-R02   | resolver: nothing configured -> empty (not_applicable)                                  |
//! | PMR-R03   | resolver: timeframe absent -> empty even if MQK_STRATEGY_SYMBOL is set                  |
//! | PMR-R04   | resolver: approved watchlist-v2 artifact supersedes the legacy single symbol            |
//! | PMR-R05   | resolver: not-approved watchlist falls back to the legacy single symbol                 |
//! | PMR-API01 | autonomous/readiness market_data_readiness present with per-symbol rows (paper+alpaca)  |
//! | PMR-API02 | autonomous/readiness market_data_readiness is null for non-paper+alpaca                 |
//! | PMR-API03 | system/preflight market_data_readiness present with per-symbol rows (paper+alpaca)      |
//! | PMR-DB01  | (DB-backed, skip without MQK_DATABASE_URL) two missing symbols -> mixed_blocked, both named |

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    api_types::MarketDataFreshnessStatus,
    market_data_freshness::{
        aggregate_freshness_statuses, evaluate_md_freshness_snapshot, evaluate_md_freshness_status,
        evaluate_md_freshness_status_for_symbols, normalize_required_symbols,
        required_symbols_for_freshness_gate_from_env, RequiredSymbolTimeframe,
    },
    routes, state,
};
use state::BrokerKind;
use tokio::sync::Mutex;

const NOW_TS: i64 = 2_000_000_000;

// ---------------------------------------------------------------------------
// Env-var serialization — tests that mutate process-global env vars
// (MQK_STRATEGY_SYMBOL / MQK_STRATEGY_MD_TIMEFRAME / MQK_PAPER_WATCHLIST_PATH /
// MQK_STRATEGY_IDS) must not run concurrently with each other or with the
// HTTP-surface tests that read them. Mirrors
// scenario_market_data_bar_context_01.rs / scenario_watchlist_readonly_handoff_01.rs.
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// MarketDataFreshnessStatus fixture helpers (built via the real pure
// evaluator, not hand-rolled literals, so thresholds stay in sync with
// market_data_freshness.rs).
// ---------------------------------------------------------------------------

fn ok_status(symbol: &str) -> MarketDataFreshnessStatus {
    evaluate_md_freshness_snapshot(symbol, "1D", 10, Some(NOW_TS - 3_600), NOW_TS)
}

fn missing_status(symbol: &str) -> MarketDataFreshnessStatus {
    evaluate_md_freshness_snapshot(symbol, "1D", 0, None, NOW_TS)
}

fn insufficient_status(symbol: &str) -> MarketDataFreshnessStatus {
    evaluate_md_freshness_snapshot(symbol, "1D", 2, Some(NOW_TS - 3_600), NOW_TS)
}

fn stale_status(symbol: &str) -> MarketDataFreshnessStatus {
    evaluate_md_freshness_snapshot(symbol, "1D", 10, Some(NOW_TS - 5 * 24 * 3_600), NOW_TS)
}

// ---------------------------------------------------------------------------
// Watchlist-v2 fixture helpers (mirrors scenario_watchlist_readonly_handoff_01.rs)
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn write_watchlist(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_pmr_watchlist_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write watchlist fixture file");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn valid_watchlist_v2(
    approved_for_paper: bool,
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
  "mode": "paper",
  "approved_for_autonomous_paper": {approved_for_paper},
  "approved_for_live": false,
  "symbols": [{syms_json}],
  "strategy_assignments": {{{assignments_json}}},
  "max_symbols_to_trade": {max_symbols},
  "max_concurrent_positions": {max_concurrent}
}}"#
    )
}

// ---------------------------------------------------------------------------
// HTTP test helpers (mirrors scenario_data_freshness_readiness_gate_01.rs)
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

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

fn make_paper_paper() -> Arc<state::AppState> {
    Arc::new(state::AppState::new())
}

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// PMR-N01..N03 — normalize_required_symbols
// ---------------------------------------------------------------------------

#[test]
fn pmr_n01_normalize_trims_uppercases_and_dedupes_symbols() {
    let required = vec![
        RequiredSymbolTimeframe {
            symbol: "aapl".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: " AAPL ".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "MSFT".to_string(),
            timeframe: "1D".to_string(),
        },
    ];
    let normalized = normalize_required_symbols(&required);
    assert_eq!(
        normalized,
        vec![
            RequiredSymbolTimeframe {
                symbol: "AAPL".to_string(),
                timeframe: "1D".to_string(),
            },
            RequiredSymbolTimeframe {
                symbol: "MSFT".to_string(),
                timeframe: "1D".to_string(),
            },
        ],
        "PMR-N01: whitespace/case duplicates of AAPL must collapse to one normalized entry"
    );
}

#[test]
fn pmr_n02_normalize_drops_entries_with_empty_symbol_or_timeframe() {
    let required = vec![
        RequiredSymbolTimeframe {
            symbol: "".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "AAPL".to_string(),
            timeframe: "   ".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "MSFT".to_string(),
            timeframe: "1D".to_string(),
        },
    ];
    let normalized = normalize_required_symbols(&required);
    assert_eq!(
        normalized.len(),
        1,
        "PMR-N02: only MSFT/1D should survive; got {normalized:?}"
    );
    assert_eq!(normalized[0].symbol, "MSFT");
}

#[test]
fn pmr_n03_normalize_keeps_distinct_timeframes_for_the_same_symbol() {
    let required = vec![
        RequiredSymbolTimeframe {
            symbol: "AAPL".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "AAPL".to_string(),
            timeframe: "5m".to_string(),
        },
    ];
    let normalized = normalize_required_symbols(&required);
    assert_eq!(
        normalized.len(),
        2,
        "PMR-N03: distinct timeframes for the same symbol are distinct requirements"
    );
}

// ---------------------------------------------------------------------------
// PMR-A01..A09 — aggregate_freshness_statuses
// ---------------------------------------------------------------------------

#[test]
fn pmr_a01_aggregate_empty_per_symbol_is_not_applicable() {
    let report = aggregate_freshness_statuses(Vec::new(), Vec::new());
    assert_eq!(report.aggregate_status, "not_applicable");
    assert!(report.start_allowed);
    assert!(report.required_symbols.is_empty());
    assert!(report.per_symbol.is_empty());
    assert!(report.blockers.is_empty());
}

#[test]
fn pmr_a02_aggregate_single_symbol_ok_matches_legacy_state() {
    let status = ok_status("AAPL");
    let expected_state = status.freshness_state.clone();
    let report = aggregate_freshness_statuses(vec!["AAPL".to_string()], vec![status]);
    assert_eq!(report.aggregate_status, expected_state);
    assert_eq!(report.aggregate_status, "ok");
    assert!(report.start_allowed);
    assert!(report.blockers.is_empty());
}

#[test]
fn pmr_a03_aggregate_single_symbol_missing_matches_legacy_state() {
    let status = missing_status("AAPL");
    let report = aggregate_freshness_statuses(vec!["AAPL".to_string()], vec![status.clone()]);
    // Backward compatibility (requirement #1): the n=1 aggregate is
    // byte-identical to the pre-existing single-symbol freshness_state /
    // is_start_blocker decision.
    assert_eq!(report.aggregate_status, status.freshness_state);
    assert_eq!(report.aggregate_status, "missing");
    assert!(!report.start_allowed);
    assert_eq!(report.blockers, vec![status.reason]);
}

#[test]
fn pmr_a04_aggregate_multi_symbol_all_ok_is_ok_and_start_allowed() {
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "MSFT".to_string()],
        vec![ok_status("AAPL"), ok_status("MSFT")],
    );
    assert_eq!(report.aggregate_status, "ok");
    assert!(report.start_allowed);
    assert!(report.blockers.is_empty());
    assert_eq!(report.per_symbol.len(), 2);
}

#[test]
fn pmr_a05_aggregate_multi_symbol_one_missing_blocks_and_names_symbol() {
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "MSFT".to_string(), "NVDA".to_string()],
        vec![ok_status("AAPL"), missing_status("MSFT"), ok_status("NVDA")],
    );
    assert_eq!(
        report.aggregate_status, "missing",
        "PMR-A05: exactly one blocker -> aggregate is that symbol's own state"
    );
    assert!(!report.start_allowed);
    assert_eq!(report.blockers.len(), 1);
    assert!(
        report.blockers[0].contains("MSFT"),
        "blocker must name MSFT; got {:?}",
        report.blockers
    );
}

#[test]
fn pmr_a06_aggregate_multi_symbol_one_stale_blocks_and_names_symbol() {
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "TSLA".to_string()],
        vec![ok_status("AAPL"), stale_status("TSLA")],
    );
    assert_eq!(report.aggregate_status, "stale");
    assert!(!report.start_allowed);
    assert_eq!(report.blockers.len(), 1);
    assert!(report.blockers[0].contains("TSLA"));
}

#[test]
fn pmr_a07_aggregate_multi_symbol_one_insufficient_blocks_and_names_symbol() {
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "NVDA".to_string()],
        vec![ok_status("AAPL"), insufficient_status("NVDA")],
    );
    assert_eq!(report.aggregate_status, "insufficient");
    assert!(!report.start_allowed);
    assert_eq!(report.blockers.len(), 1);
    assert!(report.blockers[0].contains("NVDA"));
}

#[test]
fn pmr_a08_aggregate_multi_symbol_two_blocking_is_mixed_blocked() {
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()],
        vec![ok_status("AAPL"), missing_status("MSFT"), stale_status("TSLA")],
    );
    assert_eq!(report.aggregate_status, "mixed_blocked");
    assert!(!report.start_allowed);
    assert_eq!(report.blockers.len(), 2);
    assert!(report.blockers.iter().any(|b| b.contains("MSFT")));
    assert!(report.blockers.iter().any(|b| b.contains("TSLA")));
}

#[tokio::test]
async fn pmr_a09_aggregate_multi_symbol_unavailable_without_blockers_is_unavailable() {
    let unavailable_a = evaluate_md_freshness_status(None, "AAPL", "1D", NOW_TS).await;
    let unavailable_b = evaluate_md_freshness_status(None, "MSFT", "1D", NOW_TS).await;
    let report = aggregate_freshness_statuses(
        vec!["AAPL".to_string(), "MSFT".to_string()],
        vec![unavailable_a, unavailable_b],
    );
    assert_eq!(report.aggregate_status, "unavailable");
    assert!(report.start_allowed, "PMR-A09: unavailable must not block start");
    assert!(report.blockers.is_empty());
}

// ---------------------------------------------------------------------------
// PMR-E01..E04 — evaluate_md_freshness_status_for_symbols (db-optional glue)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pmr_e01_evaluate_for_symbols_single_no_db_is_unavailable_not_blocker() {
    let required = vec![RequiredSymbolTimeframe {
        symbol: "AAPL".to_string(),
        timeframe: "1D".to_string(),
    }];
    let report = evaluate_md_freshness_status_for_symbols(None, &required, NOW_TS).await;
    assert_eq!(report.aggregate_status, "unavailable");
    assert!(report.start_allowed);
    assert_eq!(report.required_symbols, vec!["AAPL".to_string()]);
    assert_eq!(report.per_symbol.len(), 1);
}

#[tokio::test]
async fn pmr_e02_evaluate_for_symbols_empty_required_is_not_applicable() {
    let report = evaluate_md_freshness_status_for_symbols(None, &[], NOW_TS).await;
    assert_eq!(report.aggregate_status, "not_applicable");
    assert!(report.start_allowed);
    assert!(report.per_symbol.is_empty());
}

#[tokio::test]
async fn pmr_e03_evaluate_for_symbols_dedupes_whitespace_and_case_before_evaluating() {
    let required = vec![
        RequiredSymbolTimeframe {
            symbol: "aapl".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: " AAPL ".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "MSFT".to_string(),
            timeframe: "1D".to_string(),
        },
    ];
    let report = evaluate_md_freshness_status_for_symbols(None, &required, NOW_TS).await;
    assert_eq!(
        report.required_symbols,
        vec!["AAPL".to_string(), "MSFT".to_string()],
        "PMR-E03: duplicate/whitespace symbols must collapse to one evaluated entry"
    );
    assert_eq!(report.per_symbol.len(), 2);
}

#[tokio::test]
async fn pmr_e04_single_symbol_path_is_byte_identical_to_legacy_evaluator() {
    // Backward compatibility (requirement #1): with exactly one required
    // symbol, the new multi-symbol evaluator must produce an aggregate
    // status, start-allowed decision, and reason text identical to calling
    // the original single-symbol evaluate_md_freshness_status directly.
    let legacy = evaluate_md_freshness_status(None, "AAPL", "1D", NOW_TS).await;
    let required = vec![RequiredSymbolTimeframe {
        symbol: "AAPL".to_string(),
        timeframe: "1D".to_string(),
    }];
    let report = evaluate_md_freshness_status_for_symbols(None, &required, NOW_TS).await;

    assert_eq!(report.aggregate_status, legacy.freshness_state);
    assert_eq!(report.start_allowed, !legacy.is_start_blocker());
    assert_eq!(report.per_symbol[0].reason, legacy.reason);
}

// ---------------------------------------------------------------------------
// PMR-R01..R05 — required_symbols_for_freshness_gate_from_env
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pmr_r01_resolver_uses_legacy_single_symbol_when_no_watchlist_and_no_strategy_ids() {
    let _g = env_lock().lock().await;
    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        // Deliberately absent — required #-symbols resolution must not
        // depend on MQK_STRATEGY_IDS (see market_data_freshness.rs doc comment).
        std::env::remove_var("MQK_STRATEGY_IDS");
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let required = required_symbols_for_freshness_gate_from_env();

    unsafe {
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    assert_eq!(
        required,
        vec![RequiredSymbolTimeframe {
            symbol: "AAPL".to_string(),
            timeframe: "1D".to_string(),
        }],
        "PMR-R01: legacy single symbol must resolve independent of MQK_STRATEGY_IDS"
    );
}

#[tokio::test]
async fn pmr_r02_resolver_returns_empty_when_nothing_configured() {
    let _g = env_lock().lock().await;
    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    let required = required_symbols_for_freshness_gate_from_env();
    assert!(
        required.is_empty(),
        "PMR-R02: nothing configured -> empty (not_applicable)"
    );
}

#[tokio::test]
async fn pmr_r03_resolver_returns_empty_when_timeframe_absent_even_if_symbol_set() {
    let _g = env_lock().lock().await;
    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    let required = required_symbols_for_freshness_gate_from_env();

    unsafe {
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
    }

    assert!(
        required.is_empty(),
        "PMR-R03: timeframe absent -> empty, matching legacy not_applicable behavior"
    );
}

#[tokio::test]
async fn pmr_r04_resolver_prefers_approved_watchlist_v2_over_legacy_symbol() {
    let _g = env_lock().lock().await;
    let json = valid_watchlist_v2(
        true,
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        3,
        2,
    );
    let path = write_watchlist("pmr_r04", &json);

    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        // Legacy symbol present but must be superseded by the watchlist.
        std::env::set_var("MQK_STRATEGY_SYMBOL", "SPY");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
        std::env::remove_var("MQK_STRATEGY_IDS");
    }

    let required = required_symbols_for_freshness_gate_from_env();

    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }
    cleanup(&path);

    let mut symbols: Vec<String> = required.iter().map(|r| r.symbol.clone()).collect();
    symbols.sort();
    assert_eq!(
        symbols,
        vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()]
    );
    assert!(required.iter().all(|r| r.timeframe == "1D"));
    assert!(
        !required.iter().any(|r| r.symbol == "SPY"),
        "PMR-R04: watchlist-v2 must supersede the legacy single symbol"
    );
}

#[tokio::test]
async fn pmr_r05_resolver_falls_back_to_legacy_when_watchlist_not_approved() {
    let _g = env_lock().lock().await;
    let json = valid_watchlist_v2(
        false, // approved_for_autonomous_paper = false -> LoadedNotApproved
        &["AAPL", "MSFT"],
        &[("AAPL", "strat-scalper-001"), ("MSFT", "strat-scalper-002")],
        2,
        1,
    );
    let path = write_watchlist("pmr_r05", &json);

    unsafe {
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", path.to_str().unwrap());
        std::env::set_var("MQK_STRATEGY_SYMBOL", "SPY");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let required = required_symbols_for_freshness_gate_from_env();

    unsafe {
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }
    cleanup(&path);

    assert_eq!(
        required,
        vec![RequiredSymbolTimeframe {
            symbol: "SPY".to_string(),
            timeframe: "1D".to_string(),
        }],
        "PMR-R05: not-approved watchlist must fall back to the legacy single symbol"
    );
}

// ---------------------------------------------------------------------------
// PMR-API01..API03 — additive API surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pmr_api01_autonomous_readiness_includes_market_data_readiness_for_paper_alpaca() {
    let _g = env_lock().lock().await;
    let st = make_paper_alpaca();
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let readiness = &v["market_data_readiness"];
    assert!(
        !readiness.is_null(),
        "PMR-API01: paper+alpaca must return a market_data_readiness object"
    );
    assert!(readiness["aggregate_status"].is_string());
    assert!(readiness["start_allowed"].is_boolean());
    assert!(readiness["required_symbols"].is_array());
    assert!(
        readiness["per_symbol"].is_array(),
        "PMR-API01: per-symbol status rows must be present"
    );
    assert!(readiness["blockers"].is_array());
}

#[tokio::test]
async fn pmr_api02_autonomous_readiness_market_data_readiness_null_for_non_paper_alpaca() {
    let _g = env_lock().lock().await;
    let st = make_paper_paper();
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert!(
        v["market_data_readiness"].is_null(),
        "PMR-API02: non-paper+alpaca must return market_data_readiness=null"
    );
}

#[tokio::test]
async fn pmr_api03_system_preflight_includes_market_data_readiness_for_paper_alpaca() {
    let _g = env_lock().lock().await;
    let st = make_paper_alpaca();
    let router = routes::build_router(st);
    let (status, body) = call(
        router,
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    let readiness = &v["market_data_readiness"];
    assert!(
        !readiness.is_null(),
        "PMR-API03: paper+alpaca preflight must return a market_data_readiness object"
    );
    assert!(readiness["per_symbol"].is_array());
}

// ---------------------------------------------------------------------------
// PMR-DB01 — DB-backed (skip without MQK_DATABASE_URL pointing to paper DB)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pmr_db01_two_missing_symbols_aggregate_mixed_blocked_with_both_named() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("PMR-DB01: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("PMR-DB01: pool connect failed");

    let required = vec![
        RequiredSymbolTimeframe {
            symbol: "NONEXISTENT_PMR_DB01_A".to_string(),
            timeframe: "1D".to_string(),
        },
        RequiredSymbolTimeframe {
            symbol: "NONEXISTENT_PMR_DB01_B".to_string(),
            timeframe: "1D".to_string(),
        },
    ];

    let report =
        evaluate_md_freshness_status_for_symbols(Some(&pool), &required, 1_700_000_000).await;

    assert_eq!(
        report.aggregate_status, "mixed_blocked",
        "PMR-DB01: two missing symbols must aggregate to mixed_blocked"
    );
    assert!(!report.start_allowed);
    assert_eq!(report.per_symbol.len(), 2);
    assert_eq!(report.blockers.len(), 2);
    assert!(report
        .blockers
        .iter()
        .any(|b| b.contains("NONEXISTENT_PMR_DB01_A")));
    assert!(report
        .blockers
        .iter()
        .any(|b| b.contains("NONEXISTENT_PMR_DB01_B")));
    assert!(report
        .required_symbols
        .contains(&"NONEXISTENT_PMR_DB01_A".to_string()));
    assert!(report
        .required_symbols
        .contains(&"NONEXISTENT_PMR_DB01_B".to_string()));
}
