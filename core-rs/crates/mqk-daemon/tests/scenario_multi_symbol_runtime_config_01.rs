//! MULTI-SYMBOL-RUNTIME-CONFIG-01: pure multi-symbol runtime configuration
//! builder proof matrix.
//!
//! Proves [`mqk_daemon::state::MultiSymbolRuntimeConfig`] construction across
//! the legacy single-symbol fallback, the `watchlist-v2` artifact source, and
//! the selection helper that chooses between them.
//!
//! All builders under test here are pure (no env reads, no I/O) — every test
//! is a plain `#[test]`, with hand-constructed
//! [`mqk_daemon::watchlist_intake::WatchlistIntakeOutcome`] /
//! [`mqk_daemon::watchlist_intake::LoadedWatchlistArtifact`] values.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                       |
//! |------|---------------------------------------------------------------------------------------|
//! | M01  | Legacy: all three inputs present → Ok, `EnvSingleSymbolFallback`, 1 symbol, cap=1      |
//! | M02  | Legacy: missing symbol → `Err(MissingSymbol)`                                         |
//! | M03  | Legacy: missing strategy_id → `Err(MissingStrategyId)`                                |
//! | M04  | Legacy: missing timeframe → `Err(MissingTimeframe)`                                   |
//! | M05  | Legacy: whitespace-only symbol → `Err(MissingSymbol)` (trim)                          |
//! | M06  | Watchlist-v2: valid 3-symbol `LoadedApproved` → Ok, 3 assignments, cap=max_to_trade    |
//! | M07  | Watchlist: `NotConfigured` → `Err(WatchlistNotApproved)`                              |
//! | M08  | Watchlist: `LoadedNotApproved` → `Err(WatchlistNotApproved)`                          |
//! | M09  | Watchlist: `LoadedApproved` with v1 schema → `Err(WatchlistNotV2)`                    |
//! | M10  | Watchlist-v2: valid artifact, no timeframe → `Err(MissingTimeframe)`                  |
//! | M11  | Watchlist-v2: `max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING` → `Err(HardCeilingExceeded)` |
//! | M12  | Watchlist-v2: `symbols.len() > max_symbols_to_trade` → `Err(ConcurrentLimitExceeded)`  |
//! | M13  | Watchlist-v2: symbol with no `strategy_assignments` entry → `Err(MissingAssignment)`  |
//! | M14  | Selection: valid watchlist-v2 preferred over legacy                                   |
//! | M15  | Selection: `NotConfigured` watchlist → falls back to legacy                           |
//! | M16  | Selection: watchlist-v2 builder error → falls back to legacy (not an error)          |
//! | M17  | Selection: both sources fail → `Err` (fail-closed, no fabricated config)              |
//! | M18  | `MultiSymbolRuntimeConfig` carries no `approved_for_live` field (legacy + v2 paths)    |
//!
//! # Cross-references
//! - `docs/design/native_multi_symbol_dispatch.md` §4.1, §4.3, §6
//! - `docs/specs/market_scanner_backtest_candidate_pipeline.md` (Implementation Note,
//!   MULTI-SYMBOL-RUNTIME-CONFIG-01)
//! - `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs`

use std::collections::HashMap;

use mqk_daemon::state::{
    build_legacy_single_symbol_config, build_multi_symbol_config_from_watchlist_artifact,
    build_multi_symbol_runtime_config_from_env_and_watchlist, MultiSymbolConfigError,
    MultiSymbolConfigSource, SymbolStrategyAssignment, MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION,
};
use mqk_daemon::watchlist_intake::{
    LoadedWatchlistArtifact, WatchlistIntakeOutcome, MULTI_SYMBOL_HARD_CEILING,
    WATCHLIST_SCHEMA_VERSION_V1, WATCHLIST_SCHEMA_VERSION_V2,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Hand-construct a `watchlist-v2` [`LoadedWatchlistArtifact`].
fn artifact_v2(
    symbols: &[&str],
    assignments: &[(&str, &str)],
    max_symbols_to_trade: u64,
    max_concurrent_positions: u64,
) -> LoadedWatchlistArtifact {
    let symbols_vec: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
    let top_symbol = symbols_vec.first().cloned();
    let strategy_assignments: HashMap<String, String> = assignments
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    LoadedWatchlistArtifact {
        schema_version: WATCHLIST_SCHEMA_VERSION_V2.to_string(),
        symbols: symbols_vec,
        top_symbol,
        strategy_assignments,
        max_symbols_to_trade,
        max_concurrent_positions,
        approved_for_autonomous_paper: true,
    }
}

/// Hand-construct a `watchlist-v1` [`LoadedWatchlistArtifact`] (single symbol).
fn artifact_v1(symbol: &str, strategy_id: &str) -> LoadedWatchlistArtifact {
    let mut strategy_assignments = HashMap::new();
    strategy_assignments.insert(symbol.to_string(), strategy_id.to_string());
    LoadedWatchlistArtifact {
        schema_version: WATCHLIST_SCHEMA_VERSION_V1.to_string(),
        symbols: vec![symbol.to_string()],
        top_symbol: Some(symbol.to_string()),
        strategy_assignments,
        max_symbols_to_trade: 1,
        max_concurrent_positions: 1,
        approved_for_autonomous_paper: true,
    }
}

fn loaded_approved(artifact: LoadedWatchlistArtifact) -> WatchlistIntakeOutcome {
    WatchlistIntakeOutcome::LoadedApproved { artifact }
}

fn loaded_not_approved(artifact: LoadedWatchlistArtifact) -> WatchlistIntakeOutcome {
    WatchlistIntakeOutcome::LoadedNotApproved { artifact }
}

// ---------------------------------------------------------------------------
// M01 — Legacy: all present → Ok, EnvSingleSymbolFallback, 1 symbol, cap=1
// ---------------------------------------------------------------------------

#[test]
fn m01_legacy_all_present_returns_env_single_symbol_fallback() {
    let cfg =
        build_legacy_single_symbol_config(Some("AAPL"), Some("strat-scalper-001"), Some("5Min"))
            .expect("expected Ok");

    assert_eq!(cfg.schema_version, MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION);
    assert_eq!(cfg.symbols.len(), 1);
    assert_eq!(
        cfg.symbols[0],
        SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "strat-scalper-001".to_string(),
            timeframe: "5Min".to_string(),
        }
    );
    assert_eq!(cfg.max_concurrent_symbols, 1);
    assert_eq!(cfg.source, MultiSymbolConfigSource::EnvSingleSymbolFallback);
}

// ---------------------------------------------------------------------------
// M02 — Legacy: missing symbol → Err(MissingSymbol)
// ---------------------------------------------------------------------------

#[test]
fn m02_legacy_missing_symbol_fails_closed() {
    let err =
        build_legacy_single_symbol_config(None, Some("strat-scalper-001"), Some("5Min"))
            .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::MissingSymbol);
    assert_eq!(err.as_str(), "multi_symbol_config_missing_symbol");
}

// ---------------------------------------------------------------------------
// M03 — Legacy: missing strategy_id → Err(MissingStrategyId)
// ---------------------------------------------------------------------------

#[test]
fn m03_legacy_missing_strategy_id_fails_closed() {
    let err = build_legacy_single_symbol_config(Some("AAPL"), None, Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::MissingStrategyId);
    assert_eq!(err.as_str(), "multi_symbol_config_missing_strategy_id");
}

// ---------------------------------------------------------------------------
// M04 — Legacy: missing timeframe → Err(MissingTimeframe)
// ---------------------------------------------------------------------------

#[test]
fn m04_legacy_missing_timeframe_fails_closed() {
    let err = build_legacy_single_symbol_config(Some("AAPL"), Some("strat-scalper-001"), None)
        .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::MissingTimeframe);
    assert_eq!(err.as_str(), "multi_symbol_config_missing_timeframe");
}

// ---------------------------------------------------------------------------
// M05 — Legacy: whitespace-only symbol → Err(MissingSymbol) (trim)
// ---------------------------------------------------------------------------

#[test]
fn m05_legacy_whitespace_only_symbol_fails_closed() {
    let err =
        build_legacy_single_symbol_config(Some("   "), Some("strat-scalper-001"), Some("5Min"))
            .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::MissingSymbol);
}

// ---------------------------------------------------------------------------
// M06 — Watchlist-v2: valid 3-symbol LoadedApproved → Ok, 3 assignments
// ---------------------------------------------------------------------------

#[test]
fn m06_watchlist_v2_loaded_approved_builds_three_symbol_config() {
    let artifact = artifact_v2(
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        3,
        2,
    );
    let outcome = loaded_approved(artifact);

    let cfg =
        build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
            .expect("expected Ok");

    assert_eq!(cfg.schema_version, MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION);
    assert_eq!(cfg.symbols.len(), 3);
    assert_eq!(
        cfg.symbols[0],
        SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "strat-scalper-001".to_string(),
            timeframe: "5Min".to_string(),
        }
    );
    assert_eq!(
        cfg.symbols[1],
        SymbolStrategyAssignment {
            symbol: "MSFT".to_string(),
            strategy_id: "strat-scalper-002".to_string(),
            timeframe: "5Min".to_string(),
        }
    );
    assert_eq!(
        cfg.symbols[2],
        SymbolStrategyAssignment {
            symbol: "TSLA".to_string(),
            strategy_id: "strat-scalper-003".to_string(),
            timeframe: "5Min".to_string(),
        }
    );
    // max_concurrent_symbols = artifact.max_symbols_to_trade, NOT max_concurrent_positions.
    assert_eq!(cfg.max_concurrent_symbols, 3);
    assert_eq!(
        cfg.source,
        MultiSymbolConfigSource::WatchlistArtifactV2 {
            path: "/cfg/watchlist.json".to_string(),
        }
    );
}

// ---------------------------------------------------------------------------
// M07 — Watchlist: NotConfigured → Err(WatchlistNotApproved)
// ---------------------------------------------------------------------------

#[test]
fn m07_watchlist_not_configured_fails_closed_not_approved() {
    let err = build_multi_symbol_config_from_watchlist_artifact(
        &WatchlistIntakeOutcome::NotConfigured,
        "",
        Some("5Min"),
    )
    .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::WatchlistNotApproved);
    assert_eq!(err.as_str(), "multi_symbol_config_watchlist_not_approved");
}

// ---------------------------------------------------------------------------
// M08 — Watchlist: LoadedNotApproved → Err(WatchlistNotApproved)
// ---------------------------------------------------------------------------

#[test]
fn m08_watchlist_loaded_not_approved_fails_closed() {
    let artifact = artifact_v2(&["AAPL"], &[("AAPL", "strat-scalper-001")], 1, 1);
    let outcome = loaded_not_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::WatchlistNotApproved);
}

// ---------------------------------------------------------------------------
// M09 — Watchlist: LoadedApproved with v1 schema → Err(WatchlistNotV2)
// ---------------------------------------------------------------------------

#[test]
fn m09_watchlist_v1_artifact_fails_not_v2() {
    let artifact = artifact_v1("AAPL", "strat-scalper-001");
    let outcome = loaded_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::WatchlistNotV2);
    assert_eq!(err.as_str(), "multi_symbol_config_watchlist_not_v2");
}

// ---------------------------------------------------------------------------
// M10 — Watchlist-v2: valid artifact, no timeframe → Err(MissingTimeframe)
// ---------------------------------------------------------------------------

#[test]
fn m10_watchlist_v2_missing_timeframe_fails_closed() {
    let artifact = artifact_v2(&["AAPL"], &[("AAPL", "strat-scalper-001")], 1, 1);
    let outcome = loaded_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", None)
        .expect_err("expected Err");

    assert_eq!(err, MultiSymbolConfigError::MissingTimeframe);
    assert_eq!(err.as_str(), "multi_symbol_config_missing_timeframe");
}

// ---------------------------------------------------------------------------
// M11 — Watchlist-v2: max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING
//        → Err(HardCeilingExceeded)
// ---------------------------------------------------------------------------

#[test]
fn m11_watchlist_v2_over_hard_ceiling_fails_closed() {
    let over = MULTI_SYMBOL_HARD_CEILING + 1;
    let artifact = artifact_v2(&["AAPL"], &[("AAPL", "strat-scalper-001")], over, over);
    let outcome = loaded_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(
        err,
        MultiSymbolConfigError::HardCeilingExceeded {
            configured: over as usize,
            ceiling: MULTI_SYMBOL_HARD_CEILING as usize,
        }
    );
    assert_eq!(err.as_str(), "multi_symbol_config_hard_ceiling_exceeded");
}

// ---------------------------------------------------------------------------
// M12 — Watchlist-v2: symbols.len() > max_symbols_to_trade
//        → Err(ConcurrentLimitExceeded)
// ---------------------------------------------------------------------------

#[test]
fn m12_watchlist_v2_symbols_exceed_max_to_trade_fails_closed() {
    let artifact = artifact_v2(
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        2, // max_symbols_to_trade < symbols.len()
        2,
    );
    let outcome = loaded_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(
        err,
        MultiSymbolConfigError::ConcurrentLimitExceeded {
            configured: 3,
            limit: 2,
        }
    );
    assert_eq!(err.as_str(), "multi_symbol_config_concurrent_limit_exceeded");
}

// ---------------------------------------------------------------------------
// M13 — Watchlist-v2: symbol with no strategy_assignments entry
//        → Err(MissingAssignment)
// ---------------------------------------------------------------------------

#[test]
fn m13_watchlist_v2_missing_assignment_fails_closed() {
    let artifact = artifact_v2(
        &["AAPL", "MSFT"],
        &[("AAPL", "strat-scalper-001")], // MSFT has no assignment
        2,
        2,
    );
    let outcome = loaded_approved(artifact);

    let err = build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
        .expect_err("expected Err");

    assert_eq!(
        err,
        MultiSymbolConfigError::MissingAssignment {
            symbol: "MSFT".to_string(),
        }
    );
    assert_eq!(err.as_str(), "multi_symbol_config_missing_assignment");
}

// ---------------------------------------------------------------------------
// M14 — Selection: valid watchlist-v2 preferred over legacy
// ---------------------------------------------------------------------------

#[test]
fn m14_selection_prefers_valid_watchlist_v2_over_legacy() {
    let artifact = artifact_v2(
        &["AAPL", "MSFT"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
        ],
        2,
        2,
    );
    let outcome = loaded_approved(artifact);

    let cfg = build_multi_symbol_runtime_config_from_env_and_watchlist(
        &outcome,
        Some("/cfg/watchlist.json"),
        Some("TSLA"),
        Some("strat-legacy"),
        Some("5Min"),
    )
    .expect("expected Ok");

    assert_eq!(
        cfg.source,
        MultiSymbolConfigSource::WatchlistArtifactV2 {
            path: "/cfg/watchlist.json".to_string(),
        }
    );
    assert_eq!(cfg.symbols.len(), 2);
    assert_eq!(cfg.max_concurrent_symbols, 2);
}

// ---------------------------------------------------------------------------
// M15 — Selection: NotConfigured watchlist → falls back to legacy
// ---------------------------------------------------------------------------

#[test]
fn m15_selection_falls_back_to_legacy_when_watchlist_not_configured() {
    let cfg = build_multi_symbol_runtime_config_from_env_and_watchlist(
        &WatchlistIntakeOutcome::NotConfigured,
        None,
        Some("AAPL"),
        Some("strat-scalper-001"),
        Some("5Min"),
    )
    .expect("expected Ok");

    assert_eq!(cfg.source, MultiSymbolConfigSource::EnvSingleSymbolFallback);
    assert_eq!(cfg.symbols.len(), 1);
    assert_eq!(cfg.symbols[0].symbol, "AAPL");
    assert_eq!(cfg.max_concurrent_symbols, 1);
}

// ---------------------------------------------------------------------------
// M16 — Selection: watchlist-v2 builder error → falls back to legacy
//        (not an error)
// ---------------------------------------------------------------------------

#[test]
fn m16_selection_falls_back_to_legacy_when_watchlist_v2_builder_fails() {
    // 3 symbols but max_symbols_to_trade=2 -> ConcurrentLimitExceeded inside
    // the watchlist-v2 builder. Per the design doc, this falls back to the
    // legacy single-symbol path rather than propagating as an error.
    let artifact = artifact_v2(
        &["AAPL", "MSFT", "TSLA"],
        &[
            ("AAPL", "strat-scalper-001"),
            ("MSFT", "strat-scalper-002"),
            ("TSLA", "strat-scalper-003"),
        ],
        2,
        2,
    );
    let outcome = loaded_approved(artifact);

    let cfg = build_multi_symbol_runtime_config_from_env_and_watchlist(
        &outcome,
        Some("/cfg/watchlist.json"),
        Some("AAPL"),
        Some("strat-legacy"),
        Some("5Min"),
    )
    .expect("expected Ok — invalid watchlist-v2 falls back to legacy, not an error");

    assert_eq!(cfg.source, MultiSymbolConfigSource::EnvSingleSymbolFallback);
    assert_eq!(cfg.symbols.len(), 1);
    assert_eq!(cfg.symbols[0].symbol, "AAPL");
    assert_eq!(cfg.symbols[0].strategy_id, "strat-legacy");
    assert_eq!(cfg.max_concurrent_symbols, 1);
}

// ---------------------------------------------------------------------------
// M17 — Selection: both sources fail → Err (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn m17_selection_fails_closed_when_both_sources_fail() {
    let err = build_multi_symbol_runtime_config_from_env_and_watchlist(
        &WatchlistIntakeOutcome::NotConfigured,
        None,
        None, // legacy symbol also missing
        Some("strat-legacy"),
        Some("5Min"),
    )
    .expect_err("expected Err — both watchlist and legacy unavailable, fail-closed");

    assert_eq!(err, MultiSymbolConfigError::MissingSymbol);
}

// ---------------------------------------------------------------------------
// M18 — MultiSymbolRuntimeConfig carries no approved_for_live field
// ---------------------------------------------------------------------------

#[test]
fn m18_multi_symbol_runtime_config_carries_no_approved_for_live_field() {
    // Legacy path.
    let legacy_cfg =
        build_legacy_single_symbol_config(Some("AAPL"), Some("strat-scalper-001"), Some("5Min"))
            .expect("expected Ok");
    let legacy_debug = format!("{legacy_cfg:?}");
    assert!(
        !legacy_debug.contains("approved_for_live"),
        "legacy config must not carry approved_for_live: {legacy_debug}"
    );

    // Watchlist-v2 path.
    let artifact = artifact_v2(&["AAPL"], &[("AAPL", "strat-scalper-001")], 1, 1);
    let outcome = loaded_approved(artifact);
    let wl_cfg =
        build_multi_symbol_config_from_watchlist_artifact(&outcome, "/cfg/watchlist.json", Some("5Min"))
            .expect("expected Ok");
    let wl_debug = format!("{wl_cfg:?}");
    assert!(
        !wl_debug.contains("approved_for_live"),
        "watchlist config must not carry approved_for_live: {wl_debug}"
    );

    // Hard invariant from watchlist_intake: LoadedApproved never implies
    // approved_for_live=true.
    assert!(!outcome.approved_for_live());
}
