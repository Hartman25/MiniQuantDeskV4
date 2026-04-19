//! # BOOT-VALID-01 — Daemon startup truth for autonomous paper tasks
//!
//! ## Purpose
//!
//! Proves that the three autonomous background tasks (WS transport, session
//! controller, bar ticker) return the correct Some/None outcome for each
//! deployment configuration.  These outcomes are what the startup_truth log
//! block in `main.rs` observes to determine whether to emit INFO or WARN.
//!
//! ## What this file proves
//!
//! | Test  | Function                           | Config            | Expected |
//! |-------|------------------------------------|-------------------|----------|
//! | BV01  | spawn_autonomous_session_controller | Paper+Alpaca     | Some     |
//! | BV02  | spawn_autonomous_session_controller | LiveShadow+Alpaca | None    |
//! | BV03  | spawn_autonomous_bar_ticker         | Paper+Alpaca     | Some     |
//! | BV04  | spawn_autonomous_bar_ticker         | Paper+Paper      | None     |
//! | BV05  | symmetric: both start or neither   | Paper+Alpaca     | both Some |
//! | BV06  | symmetric: neither when non-paper  | LiveShadow+Alpaca | both None |
//!
//! ## What is NOT claimed
//!
//! - The `warn!` log text itself (log output is not wired in unit/scenario tests).
//! - WS transport credential checks — those require real env vars and are
//!   already proven by the BRK-00R-05 transport tests.
//! - DB-backed runtime behavior of the tasks once started.
//!
//! All tests are pure in-process.  No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use mqk_daemon::state::{
    spawn_autonomous_bar_ticker, spawn_autonomous_session_controller, AppState, BrokerKind,
    DeploymentMode,
};

// ---------------------------------------------------------------------------
// BV01: Session controller spawns for Paper+Alpaca
// ---------------------------------------------------------------------------
//
// spawn_autonomous_session_controller returns Some only when deployment=Paper
// AND strategy_market_data_source=ExternalSignalIngestion (paper+alpaca).
// If it returned None for a paper+alpaca deployment, the startup_truth block
// in main.rs would emit a warn about asymmetric task outcomes.

#[tokio::test]
async fn bv01_session_controller_starts_for_paper_alpaca() {
    let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let handle = spawn_autonomous_session_controller(Arc::clone(&state));
    assert!(
        handle.is_some(),
        "BV01: spawn_autonomous_session_controller must return Some for paper+alpaca — \
         if None, startup_truth warn fires at daemon boot indicating a config inconsistency"
    );
    // handle dropped here; spawned task aborted cleanly.
}

// ---------------------------------------------------------------------------
// BV02: Session controller skipped for non-paper (LiveShadow+Alpaca)
// ---------------------------------------------------------------------------
//
// LiveShadow is not a paper deployment; the session controller is paper-only.
// None is the expected and correct outcome here — no startup_truth warn fires.

#[tokio::test]
async fn bv02_session_controller_skipped_for_non_paper() {
    let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));
    let handle = spawn_autonomous_session_controller(Arc::clone(&state));
    assert!(
        handle.is_none(),
        "BV02: spawn_autonomous_session_controller must return None for LiveShadow+Alpaca — \
         autonomous session management is paper-only"
    );
}

// ---------------------------------------------------------------------------
// BV03: Bar ticker spawns for Paper+Alpaca
// ---------------------------------------------------------------------------
//
// spawn_autonomous_bar_ticker returns Some only when
// strategy_market_data_source=ExternalSignalIngestion (paper+alpaca).
// If it returned None for paper+alpaca, startup_truth would warn.

#[tokio::test]
async fn bv03_bar_ticker_starts_for_paper_alpaca() {
    let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let handle = spawn_autonomous_bar_ticker(Arc::clone(&state));
    assert!(
        handle.is_some(),
        "BV03: spawn_autonomous_bar_ticker must return Some for paper+alpaca — \
         if None, startup_truth warn fires at daemon boot"
    );
    // handle dropped here; spawned task aborted cleanly.
}

// ---------------------------------------------------------------------------
// BV04: Bar ticker skipped for Paper+Paper (ExternalSignalIngestion not wired)
// ---------------------------------------------------------------------------
//
// Paper+Paper uses the LockedPaperBroker (blocked at readiness gate) and has
// no ExternalSignalIngestion.  The bar ticker returns None — correct and
// expected; no startup_truth warn fires because WS also returns None.

#[tokio::test]
async fn bv04_bar_ticker_skipped_for_paper_paper() {
    let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Paper));
    let handle = spawn_autonomous_bar_ticker(Arc::clone(&state));
    assert!(
        handle.is_none(),
        "BV04: spawn_autonomous_bar_ticker must return None for paper+paper \
         (ExternalSignalIngestion is not configured; LockedPaperBroker has no bar feed)"
    );
}

// ---------------------------------------------------------------------------
// BV05: Symmetric — both session controller and bar ticker start for paper+alpaca
// ---------------------------------------------------------------------------
//
// Proves the invariant that the startup_truth INFO path in main.rs is reachable:
// when paper+alpaca is properly configured, all autonomous tasks return Some
// together (no asymmetric warn).

#[tokio::test]
async fn bv05_both_autonomous_tasks_start_symmetrically_for_paper_alpaca() {
    let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let ctrl = spawn_autonomous_session_controller(Arc::clone(&state));
    let ticker = spawn_autonomous_bar_ticker(Arc::clone(&state));
    assert!(
        ctrl.is_some() && ticker.is_some(),
        "BV05: session controller and bar ticker must both start for paper+alpaca — \
         asymmetric None is the config inconsistency that startup_truth warns about"
    );
    // handles dropped; tasks aborted cleanly.
}

// ---------------------------------------------------------------------------
// BV06: Symmetric — both skipped for non-paper (no spurious warn)
// ---------------------------------------------------------------------------
//
// For LiveShadow+Alpaca, both tasks return None.  The startup_truth block
// in main.rs only warns when WS started but controller/ticker did not.
// Since WS also returns None for LiveShadow (not paper), no warn is emitted —
// this test proves that the non-paper all-None outcome is coherent.

#[tokio::test]
async fn bv06_both_autonomous_tasks_skipped_symmetrically_for_non_paper() {
    let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));
    let ctrl = spawn_autonomous_session_controller(Arc::clone(&state));
    let ticker = spawn_autonomous_bar_ticker(Arc::clone(&state));
    assert!(
        ctrl.is_none() && ticker.is_none(),
        "BV06: session controller and bar ticker must both be None for LiveShadow+Alpaca — \
         both are paper-only; no startup_truth warn should fire"
    );
}
