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
