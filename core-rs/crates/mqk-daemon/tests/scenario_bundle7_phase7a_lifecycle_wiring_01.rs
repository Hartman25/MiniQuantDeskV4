//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: external, public-API-only
//! lifecycle-wiring proof.
//!
//! ## Why this file is small
//!
//! An earlier version of this file attempted a full
//! `start_execution_runtime()` -> `stop_execution_runtime()`/
//! `halt_execution_runtime()`/`stop_for_shutdown()` cycle against
//! Paper+Paper (`BrokerKind::Paper`), reasoning that `LockedPaperBroker` is
//! in-process and network-free. Running it against the real isolated test DB
//! proved that reasoning wrong: `deployment_mode_readiness` in
//! `state/env.rs` explicitly refuses `(Paper, Paper)` with
//! `runtime.start_refused.deployment_mode_unproven` — LockedPaperBroker "has
//! no market-data source wired in the daemon runtime and cannot produce real
//! fills... the only authoritative paper execution path is Paper+Alpaca."
//!
//! That gate runs *before* `create_or_reuse_run_for_start` — i.e. before any
//! of this patch's new code is even reached. The only deployment/broker
//! pairs that pass it are `(Paper, Alpaca)` and the Live modes with Alpaca,
//! every one of which requires constructing a real `AlpacaBrokerAdapter`
//! (real credentials) whose first orchestrator tick makes a real
//! `fetch_events` REST call to Alpaca on every tick regardless of run state
//! (see the `H06` test in `scenario_clear_halted_run_auton04.rs` for the
//! same constraint independently documented). This patch's operating rules
//! forbid loading Alpaca credentials or making network calls, so a genuine
//! executed proof of a full real `start_execution_runtime()` success ->
//! commit -> stop/halt/shutdown/reap cycle is not achievable here without
//! either live Alpaca paper credentials (forbidden) or a mock-Alpaca-server
//! test harness (a real scope addition, not attempted in this patch — see
//! the Phase 7A handoff's named PARTIAL evidence).
//!
//! What *is* proven, with real executed tests, elsewhere:
//! - Disposition-determination logic (`Off`/`ShadowInvalid`/refusal,
//!   live-lock, zero-I/O for `Off`) via `build_dynamic_selection_start_
//!   snapshot` called directly — `state::lifecycle::
//!   dynamic_selection_start_snapshot_tests` (`cargo test -p mqk-daemon
//!   --lib`), no DB, no credentials.
//! - The cleanup contract (stop/halt/shutdown clear committed
//!   dynamic-selection state, idempotently, even when the surrounding call
//!   itself errors on a missing DB) via directly-committed fixture state
//!   (bypassing the credential-gated start path entirely, since
//!   `commit_dynamic_selection_runtime_state` is `pub(crate)`) —
//!   `state::lifecycle::dynamic_selection_cleanup_contract_tests` (`cargo
//!   test -p mqk-daemon --lib`), no DB, no credentials.
//!
//! This file keeps only what is genuinely provable through the public API
//! without a DB or credentials: constructor initialization.

use mqk_daemon::state::{AppState, BrokerKind, DeploymentMode};

/// Every `AppState` constructor routes through `new_inner`, which
/// initializes `dynamic_selection_runtime` to `None`.
#[tokio::test]
async fn constructors_initialize_dynamic_selection_runtime_to_none() {
    assert!(AppState::new()
        .dynamic_selection_runtime_snapshot()
        .await
        .is_none());
    assert!(AppState::new_for_test_with_mode(DeploymentMode::Paper)
        .dynamic_selection_runtime_snapshot()
        .await
        .is_none());
    assert!(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca)
        .dynamic_selection_runtime_snapshot()
        .await
        .is_none());
    assert!(
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca)
            .dynamic_selection_runtime_snapshot()
            .await
            .is_none()
    );
}
