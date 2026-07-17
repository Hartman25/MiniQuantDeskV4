//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D1: canonical shared runtime-context
//! resolver.
//!
//! Extracts the exact B1A native-strategy bootstrap / effective-binding
//! resolution block that was previously inlined in
//! `AppState::start_execution_runtime` (`state/lifecycle.rs`) into one
//! production-safe, side-effect-free seam, so a future daily coordinator
//! (Phase D2+) can resolve the same binding identity without re-deriving it
//! independently.
//!
//! Behavior-preserving only: `start_execution_runtime` calls
//! [`resolve_autonomous_runtime_context`] in place of its former inline
//! block, with the exact same fault classes, gate order, and message text.
//! This resolver reuses `mqk_runtime::native_strategy::bootstrap_with_effective_binding`
//! — the same shared helper that already performs
//! `build_daemon_plugin_registry_and_symbol` -> `NativeStrategyBootstrap::bootstrap`
//! -> `effective_binding_from_bootstrap` in one call, from one registry
//! construction and one `MQK_STRATEGY_SYMBOL` read — so the target symbol is
//! never independently re-read or guessed here.
//!
//! This module performs:
//! - no DB access;
//! - no provider or broker call;
//! - no `AppState` mutation (the resolved bootstrap/binding are returned to
//!   the caller, never stored here);
//! - no run creation;
//! - no wall-clock read.

use super::{AppState, BrokerKind, DeploymentMode, RuntimeLifecycleError};

use mqk_runtime::native_strategy::{
    bootstrap_with_effective_binding, EffectiveRuntimeBinding, NativeStrategyBootstrap,
};

/// Resolved native-strategy bootstrap and its derived effective runtime
/// binding, produced by exactly one registry-construction call.
pub struct ResolvedAutonomousRuntimeContext {
    pub native_strategy_bootstrap: NativeStrategyBootstrap,
    pub effective_runtime_binding: EffectiveRuntimeBinding,
}

// `NativeStrategyBootstrap` does not derive `Debug` (it is not a `#[derive]`d
// type upstream in `mqk-runtime`), so this is a manual, narrow impl over its
// own public `truth_state()` label rather than widening that type's trait
// surface from this crate.
impl std::fmt::Debug for ResolvedAutonomousRuntimeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAutonomousRuntimeContext")
            .field(
                "native_strategy_bootstrap_truth_state",
                &self.native_strategy_bootstrap.truth_state(),
            )
            .field("effective_runtime_binding", &self.effective_runtime_binding)
            .finish()
    }
}

/// B1A: resolve the native strategy bootstrap and effective runtime binding.
///
/// Preserves, byte-for-byte, the fault classes and messages `lifecycle.rs`
/// emitted before this extraction:
///   - `runtime.start_refused.native_strategy_bootstrap_failed` (Failed bootstrap)
///   - `runtime.start_refused.strategy_bootstrap_dormant` (Paper+Alpaca Dormant)
///
/// Dormant is allowed through for every other deployment/broker combination
/// (e.g. LiveShadow monitor-only operation), matching the pre-extraction
/// `lifecycle.rs` scope exactly.
pub async fn resolve_autonomous_runtime_context(
    state: &AppState,
) -> Result<ResolvedAutonomousRuntimeContext, RuntimeLifecycleError> {
    let fleet_ids = state.strategy_fleet_snapshot().await.map(|entries| {
        entries
            .into_iter()
            .map(|e| e.strategy_id)
            .collect::<Vec<_>>()
    });
    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: one registry-construction call
    // captures both the bootstrap and the target-symbol read together, so the
    // effective binding derived below can never disagree with a second,
    // independently-read symbol value.
    let (native_strategy_bootstrap, effective_runtime_binding) =
        bootstrap_with_effective_binding(fleet_ids.as_deref());

    if native_strategy_bootstrap.is_failed() {
        return Err(RuntimeLifecycleError::forbidden(
            "runtime.start_refused.native_strategy_bootstrap_failed",
            "native_strategy_bootstrap",
            format!(
                "native strategy bootstrap failed (truth_state='{}'): {}; \
                 ensure the strategy named in MQK_STRATEGY_IDS is registered \
                 in the daemon plugin registry before starting; \
                 operators must not set MQK_STRATEGY_IDS until the target \
                 strategy engine is wired into the registry",
                native_strategy_bootstrap.truth_state(),
                native_strategy_bootstrap
                    .failure_reason()
                    .unwrap_or("unknown"),
            ),
        ));
    }

    // STRATEGY-DORMANCY-01: Paper+Alpaca autonomous path requires an active
    // strategy bootstrap. Dormant is allowed for non-paper deployments (e.g.
    // LiveShadow running in monitor-only mode); this block is scoped to
    // Paper+Alpaca only, matching the pre-extraction gate exactly.
    if native_strategy_bootstrap.is_dormant()
        && state.deployment_mode() == DeploymentMode::Paper
        && state.runtime_selection().broker_kind == Some(BrokerKind::Alpaca)
    {
        return Err(RuntimeLifecycleError::forbidden(
            "runtime.start_refused.strategy_bootstrap_dormant",
            "native_strategy_bootstrap",
            "paper+alpaca autonomous path requires an active strategy bootstrap; \
             MQK_STRATEGY_IDS is absent or empty — no strategy engine will generate \
             decisions; set MQK_STRATEGY_IDS to a registered strategy name \
             (e.g. 'swing_momentum') and ensure it is enabled in \
             sys_strategy_registry before starting the autonomous paper path \
             (STRATEGY-DORMANCY-01)",
        ));
    }

    Ok(ResolvedAutonomousRuntimeContext {
        native_strategy_bootstrap,
        effective_runtime_binding,
    })
}
