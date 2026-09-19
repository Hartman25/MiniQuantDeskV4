//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 (R1B): `watchlist-v3` explicit
//! per-symbol multi-strategy artifact evaluator.
//!
//! Proves the frozen contract
//! (`docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`):
//! `strategy_assignments` is `symbol -> Vec<String>`, v1/v2 parsing is
//! byte-for-byte unchanged, and `NotV3` is a routing signal, never a
//! validation failure.
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | V01 | No env var → NotConfigured |
//! | V02 | Missing file → Missing |
//! | V03 | Malformed JSON → Invalid |
//! | V04 | v1 artifact fed to v3 evaluator → NotV3 (routing, not Invalid) |
//! | V05 | v2 artifact fed to v3 evaluator → NotV3 |
//! | V06 | Valid v3, 2 symbols, AAPL has 2 strategies → LoadedApproved |
//! | V07 | approved_for_live=true → Invalid (hard live lock still applies) |
//! | V08 | mode != paper → Invalid |
//! | V09 | Symbol with empty strategy list → Invalid (missing) |
//! | V10 | Symbol with no strategy_assignments entry → Invalid (missing) |
//! | V11 | Symbol with > MAX_STRATEGIES_PER_SYMBOL entries → Invalid (ceiling exceeded) |
//! | V12 | approved_for_autonomous_paper=false → LoadedNotApproved |
//! | V13 | Strategy list order is preserved exactly (not re-sorted) |
//! | V14 | symbols.len() > max_symbols_to_trade → truncates, surfaces dropped tail |
//! | V15 | max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING → Invalid |
//! | V16 | v1 evaluator (existing `evaluate_watchlist_intake`) on a v3 file → Invalid, unchanged behavior |
//! | V17 | Singleton strategy list (1 entry) is a valid, ordinary v3 assignment |

use mqk_daemon::watchlist_intake::{
    evaluate_watchlist_intake, evaluate_watchlist_intake_v3, LoadedWatchlistArtifactV3,
    WatchlistIntakeOutcome, WatchlistIntakeOutcomeV3, MAX_STRATEGIES_PER_SYMBOL,
    MULTI_SYMBOL_HARD_CEILING,
};

use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

fn write_watchlist(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_watchlist_v3_{tag}_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(&path, contents).expect("write watchlist file");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Build a `watchlist-v3` JSON string. `strategy_assignments` is
/// `(symbol, [strategy_ids...])` pairs, preserving list order exactly.
fn valid_watchlist_v3(
    approved_for_paper: bool,
    approved_for_live: bool,
    mode: &str,
    symbols: &[&str],
    strategy_assignments: &[(&str, &[&str])],
    max_symbols: u64,
    max_concurrent: u64,
) -> String {
    let syms_json = symbols
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let assignments_json = strategy_assignments
        .iter()
        .map(|(sym, strats)| {
            let list = strats
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{sym:?}: [{list}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"{{
  "schema_version": "watchlist-v3",
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

fn approved_v3_two_symbols() -> String {
    valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL", "MSFT"],
        &[
            ("AAPL", &["intraday_scalper", "intraday_short_scalper"]),
            ("MSFT", &["swing_momentum"]),
        ],
        2,
        2,
    )
}

fn v1_watchlist() -> String {
    r#"{
  "schema_version": "watchlist-v1",
  "mode": "paper",
  "approved_for_autonomous_paper": true,
  "approved_for_live": false,
  "symbols": ["AAPL"],
  "strategy_assignments": {"AAPL": "intraday_scalper"},
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}"#
    .to_string()
}

fn v2_watchlist() -> String {
    r#"{
  "schema_version": "watchlist-v2",
  "mode": "paper",
  "approved_for_autonomous_paper": true,
  "approved_for_live": false,
  "symbols": ["AAPL", "MSFT"],
  "strategy_assignments": {"AAPL": "intraday_scalper", "MSFT": "swing_momentum"},
  "max_symbols_to_trade": 2,
  "max_concurrent_positions": 2
}"#
    .to_string()
}

// ---------------------------------------------------------------------------
// V01-V03: baseline outcomes
// ---------------------------------------------------------------------------

#[test]
fn v01_no_env_configured_returns_not_configured() {
    assert_eq!(
        evaluate_watchlist_intake_v3(None),
        WatchlistIntakeOutcomeV3::NotConfigured
    );
}

#[test]
fn v02_missing_file_returns_missing() {
    let path = std::env::temp_dir().join(format!("mqk_v3_missing_{}", next_id()));
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    assert!(matches!(outcome, WatchlistIntakeOutcomeV3::Missing { .. }));
}

#[test]
fn v03_malformed_json_returns_invalid() {
    let path = write_watchlist("v03", "{ not valid json !!!");
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert!(matches!(outcome, WatchlistIntakeOutcomeV3::Invalid { .. }));
}

// ---------------------------------------------------------------------------
// V04-V05: NotV3 routing (not a validation failure)
// ---------------------------------------------------------------------------

#[test]
fn v04_v1_artifact_fed_to_v3_evaluator_is_not_v3_not_invalid() {
    let path = write_watchlist("v04", &v1_watchlist());
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert_eq!(outcome, WatchlistIntakeOutcomeV3::NotV3);
}

#[test]
fn v05_v2_artifact_fed_to_v3_evaluator_is_not_v3_not_invalid() {
    let path = write_watchlist("v05", &v2_watchlist());
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert_eq!(outcome, WatchlistIntakeOutcomeV3::NotV3);
}

// ---------------------------------------------------------------------------
// V06: the happy path
// ---------------------------------------------------------------------------

#[test]
fn v06_valid_v3_two_symbols_aapl_two_strategies_is_loaded_approved() {
    let path = write_watchlist("v06", &approved_v3_two_symbols());
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::LoadedApproved { artifact } = outcome else {
        panic!("expected LoadedApproved, got {outcome:?}");
    };
    assert_eq!(artifact.schema_version, "watchlist-v3");
    assert_eq!(artifact.symbols, vec!["AAPL", "MSFT"]);
    assert_eq!(
        artifact.strategy_assignments.get("AAPL").unwrap(),
        &vec!["intraday_scalper".to_string(), "intraday_short_scalper".to_string()]
    );
    assert_eq!(
        artifact.strategy_assignments.get("MSFT").unwrap(),
        &vec!["swing_momentum".to_string()]
    );
}

// ---------------------------------------------------------------------------
// V07-V08: hard-lock / mode invariants unchanged for v3
// ---------------------------------------------------------------------------

#[test]
fn v07_approved_for_live_true_is_hard_locked_invalid() {
    let json = valid_watchlist_v3(
        true,
        true,
        "paper",
        &["AAPL"],
        &[("AAPL", &["intraday_scalper"])],
        1,
        1,
    );
    let path = write_watchlist("v07", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert!(matches!(outcome, WatchlistIntakeOutcomeV3::Invalid { .. }));
}

#[test]
fn v08_mode_not_paper_returns_invalid() {
    let json = valid_watchlist_v3(
        true,
        false,
        "live",
        &["AAPL"],
        &[("AAPL", &["intraday_scalper"])],
        1,
        1,
    );
    let path = write_watchlist("v08", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert!(matches!(outcome, WatchlistIntakeOutcomeV3::Invalid { .. }));
}

// ---------------------------------------------------------------------------
// V09-V11: per-symbol strategy-list bound (§2.1)
// ---------------------------------------------------------------------------

#[test]
fn v09_symbol_with_empty_strategy_list_is_invalid() {
    let json = valid_watchlist_v3(true, false, "paper", &["AAPL"], &[("AAPL", &[])], 1, 1);
    let path = write_watchlist("v09", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::Invalid { failure_reasons } = outcome else {
        panic!("expected Invalid, got {outcome:?}");
    };
    assert!(failure_reasons
        .iter()
        .any(|r| r.contains("watchlist_strategy_assignment_missing")));
}

#[test]
fn v10_symbol_with_no_assignment_entry_is_invalid() {
    let json = valid_watchlist_v3(true, false, "paper", &["AAPL"], &[], 1, 1);
    let path = write_watchlist("v10", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::Invalid { failure_reasons } = outcome else {
        panic!("expected Invalid, got {outcome:?}");
    };
    assert!(failure_reasons
        .iter()
        .any(|r| r.contains("watchlist_strategy_assignment_missing")));
}

#[test]
fn v11_symbol_over_max_strategies_per_symbol_is_invalid() {
    assert_eq!(MAX_STRATEGIES_PER_SYMBOL, 3, "test assumes the frozen bound of 3");
    let json = valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL"],
        &[("AAPL", &["s1", "s2", "s3", "s4"])],
        1,
        1,
    );
    let path = write_watchlist("v11", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::Invalid { failure_reasons } = outcome else {
        panic!("expected Invalid, got {outcome:?}");
    };
    assert!(failure_reasons
        .iter()
        .any(|r| r.contains("watchlist_strategy_per_symbol_ceiling_exceeded")));
}

// ---------------------------------------------------------------------------
// V12: not-approved
// ---------------------------------------------------------------------------

#[test]
fn v12_not_approved_returns_loaded_not_approved() {
    let json = valid_watchlist_v3(
        false,
        false,
        "paper",
        &["AAPL"],
        &[("AAPL", &["intraday_scalper"])],
        1,
        1,
    );
    let path = write_watchlist("v12", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    assert!(matches!(
        outcome,
        WatchlistIntakeOutcomeV3::LoadedNotApproved { .. }
    ));
}

// ---------------------------------------------------------------------------
// V13: order preservation
// ---------------------------------------------------------------------------

#[test]
fn v13_strategy_list_order_is_preserved_exactly() {
    let json = valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL"],
        &[("AAPL", &["zzz_strategy", "aaa_strategy"])],
        1,
        1,
    );
    let path = write_watchlist("v13", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::LoadedApproved { artifact } = outcome else {
        panic!("expected LoadedApproved, got {outcome:?}");
    };
    assert_eq!(
        artifact.strategy_assignments.get("AAPL").unwrap(),
        &vec!["zzz_strategy".to_string(), "aaa_strategy".to_string()],
        "artifact JSON array order must be preserved, never re-sorted"
    );
}

// ---------------------------------------------------------------------------
// V14: truncate-and-surface, mirrors v2's cap #1 behavior
// ---------------------------------------------------------------------------

#[test]
fn v14_over_cap_truncates_and_surfaces_dropped_tail() {
    let json = valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL", "MSFT", "NVDA"],
        &[
            ("AAPL", &["intraday_scalper"]),
            ("MSFT", &["swing_momentum"]),
            ("NVDA", &["mean_reversion"]),
        ],
        2,
        2,
    );
    let path = write_watchlist("v14", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::LoadedApproved { artifact } = outcome else {
        panic!("expected LoadedApproved, got {outcome:?}");
    };
    assert_eq!(artifact.symbols, vec!["AAPL", "MSFT"]);
    assert_eq!(artifact.dropped_symbols, vec!["NVDA"]);
}

// ---------------------------------------------------------------------------
// V15: hard ceiling
// ---------------------------------------------------------------------------

#[test]
fn v15_max_symbols_over_hard_ceiling_is_invalid() {
    let over = MULTI_SYMBOL_HARD_CEILING + 1;
    let json = valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL"],
        &[("AAPL", &["intraday_scalper"])],
        over,
        1,
    );
    let path = write_watchlist("v15", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::Invalid { failure_reasons } = outcome else {
        panic!("expected Invalid, got {outcome:?}");
    };
    assert!(failure_reasons
        .iter()
        .any(|r| r.contains("watchlist_multi_symbol_ceiling_exceeded")));
}

// ---------------------------------------------------------------------------
// V16: existing v1/v2-only evaluator is completely unaffected by v3's
// existence — a v3 file remains "unsupported schema_version" to it, exactly
// as it was before this patch.
// ---------------------------------------------------------------------------

#[test]
fn v16_existing_v1v2_evaluator_treats_v3_file_as_unsupported_schema_unchanged() {
    let path = write_watchlist("v16", &approved_v3_two_symbols());
    let outcome = evaluate_watchlist_intake(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcome::Invalid { failure_reasons } = outcome else {
        panic!("expected Invalid, got {outcome:?}");
    };
    assert!(failure_reasons
        .iter()
        .any(|r| r.contains("watchlist_schema_invalid")));
}

// ---------------------------------------------------------------------------
// V17: singleton list is an ordinary, valid case (not special-cased)
// ---------------------------------------------------------------------------

#[test]
fn v17_singleton_strategy_list_is_ordinary_valid_assignment() {
    let json = valid_watchlist_v3(
        true,
        false,
        "paper",
        &["AAPL"],
        &[("AAPL", &["intraday_scalper"])],
        1,
        1,
    );
    let path = write_watchlist("v17", &json);
    let outcome = evaluate_watchlist_intake_v3(Some(&path));
    cleanup(&path);
    let WatchlistIntakeOutcomeV3::LoadedApproved { artifact } = outcome else {
        panic!("expected LoadedApproved, got {outcome:?}");
    };
    let expected: LoadedWatchlistArtifactV3 = artifact.clone();
    assert_eq!(expected.strategy_assignments.get("AAPL").unwrap().len(), 1);
}
