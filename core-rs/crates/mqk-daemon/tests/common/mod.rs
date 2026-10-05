//! Shared fixtures for `mqk-daemon` scenario tests.

use std::path::PathBuf;

use mqk_daemon::state::AppState;

/// Absolute path to the repository's canonical legacy Equity registry.
///
/// `AppState::instrument_registry_path` defaults to the CWD-relative
/// `config/instruments/equities.json` (production launchers pin the daemon's
/// CWD to the repo root). Cargo runs integration tests with CWD = the crate
/// directory, so a test that reaches the decision seam's legacy-Equity
/// authority check must anchor the registry explicitly instead of inheriting
/// that default.
pub fn canonical_equity_registry_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("canonical equity registry must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .into_owned()
}

/// `state` with its legacy-Equity registry anchored on the repo's canonical file.
pub fn with_canonical_equity_registry(mut state: AppState) -> AppState {
    state.instrument_registry_path = canonical_equity_registry_path();
    state
}

/// Path to a copy of the canonical registry with `symbol` added as an enabled
/// legacy Equity (AAPL row cloned), so a per-run synthetic symbol satisfies the
/// seam's Equity authority check genuinely.
#[allow(dead_code)] // used by a subset of test binaries
pub fn registry_path_with_synthetic_symbol(symbol: &str) -> String {
    let dir = std::env::temp_dir().join(format!("mqk_registry_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create registry fixture dir");
    let mut rows: Vec<serde_json::Value> = serde_json::from_slice(
        &std::fs::read(canonical_equity_registry_path()).expect("read registry"),
    )
    .expect("parse registry");
    let mut row = rows
        .iter()
        .find(|r| r["symbol"] == "AAPL")
        .cloned()
        .expect("canonical registry has an AAPL row");
    row["symbol"] = symbol.into();
    row["provider_symbol"] = symbol.into();
    row["instrument_id"] = format!("equity:US:{symbol}").into();
    rows.push(row);
    let path = dir.join("equities_with_synthetic_symbol.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&rows).unwrap())
        .expect("write registry fixture");
    path.to_string_lossy().into_owned()
}
