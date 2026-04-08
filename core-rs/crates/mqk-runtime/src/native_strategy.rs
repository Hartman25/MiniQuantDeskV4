//! B1A/B1B: Native strategy runtime bootstrap seam and input bridge.
//!
//! Selects and instantiates exactly one native strategy from canonical fleet
//! truth (strategy_fleet / MQK_STRATEGY_IDS) and a [`PluginRegistry`].
//!
//! # Bootstrap truth states
//!
//! | State   | Condition                            | Start-gate action        |
//! |---------|--------------------------------------|--------------------------|
//! | Dormant | Fleet absent or empty                | Pass (not an error)      |
//! | Active  | Fleet entry found and instantiated   | Pass; host held dormant  |
//! | Failed  | Fleet entry present, not in registry | Refuse (fail-closed)     |
//!
//! # B1B: Input bridge (ExternalSignalIngestion → on_bar)
//!
//! [`build_signal_context`] converts a validated operator signal (symbol, side,
//! qty, limit_price) into a [`mqk_strategy::StrategyContext`] containing a single
//! bar stub.  [`NativeStrategyBootstrap::invoke_on_bar_from_signal`] dispatches
//! that context to the active strategy host's `on_bar` callback.
//!
//! # B1B canonical dispatch: execution loop tick
//!
//! `on_bar` fires from the execution loop, not from the HTTP route handler.
//! The signal route deposits bar input into `AppState::pending_strategy_bar_input`
//! (in `mqk-daemon`); on each tick, `AppState::tick_strategy_dispatch` takes the
//! pending input and calls [`NativeStrategyBootstrap::invoke_on_bar_from_signal`].
//! The execution loop is the authoritative dispatch owner.
//!
//! Input truth: the operator signal payload provides price/qty fields.  No
//! fabricated historical bars are constructed.  Strategies requiring multi-bar
//! lookback will return empty targets — correct conservative behavior, not an error.
//!
//! # B1C: Decision submission bridge
//!
//! B1C removes the shadow-mode constraint that was held in B1A/B1B.  The host
//! is now initialised with [`ShadowMode::Off`], producing [`mqk_strategy::IntentMode::Live`]
//! results.  The execution loop in `mqk-daemon` calls `bar_result_to_decisions`
//! to translate the result into [`InternalStrategyDecision`]s, then submits
//! each through [`submit_internal_strategy_decision`] (the canonical 7-gate
//! admission seam).  Shadow-mode results (none expected, but safe to receive)
//! are dropped without enqueue — fail-closed.
//!
//! # NOT wired after B1C
//! - bar / market-data ingestion loop (multi-bar history)
//! - multi-strategy fleet execution

use mqk_strategy::{BarStub, PluginRegistry, RecentBarsWindow, ShadowMode, StrategyBarResult,
    StrategyContext, StrategyHost};

// ---------------------------------------------------------------------------
// Bootstrap outcome
// ---------------------------------------------------------------------------

/// Truth state of the native strategy runtime bootstrap for one execution run.
pub enum NativeStrategyBootstrapOutcome {
    /// No strategy fleet is configured (MQK_STRATEGY_IDS absent or empty).
    /// The strategy runtime is dormant for this run. Not an error.
    Dormant,

    /// Exactly one strategy was selected from the fleet and successfully
    /// instantiated from the plugin registry.
    ///
    /// The host is held in shadow mode: bar ingestion is not yet wired (B1A
    /// constraint). Shadow mode is set on the host until the bar ingestion
    /// bridge is connected in a subsequent patch.
    Active {
        host: StrategyHost,
        strategy_id: String,
    },

    /// A fleet entry is present but the named strategy is not registered in
    /// the plugin registry. Fail-closed: the daemon must not start with an
    /// unresolvable strategy configuration.
    Failed {
        strategy_id: String,
        reason: String,
    },
}

/// Native strategy runtime bootstrap handle for one execution run.
///
/// Constructed at execution-run start time from canonical fleet truth and the
/// daemon plugin registry via [`NativeStrategyBootstrap::bootstrap`].
///
/// The bootstrap is stored in `AppState` from run-start to run-stop/halt.
/// `None` in `AppState` means no run is active.
pub struct NativeStrategyBootstrap {
    pub outcome: NativeStrategyBootstrapOutcome,
}

impl NativeStrategyBootstrap {
    /// Bootstrap a native strategy host from fleet IDs and a plugin registry.
    ///
    /// # Selection policy
    /// - `fleet_ids` is `None` or empty → [`Dormant`](NativeStrategyBootstrapOutcome::Dormant).
    /// - First fleet entry found in `registry` → [`Active`](NativeStrategyBootstrapOutcome::Active).
    /// - First fleet entry not in `registry` → [`Failed`](NativeStrategyBootstrapOutcome::Failed).
    ///
    /// Only the first fleet entry is consumed (single-strategy Tier A policy).
    /// Multi-strategy fleet execution is deferred to a later patch.
    pub fn bootstrap(fleet_ids: Option<&[String]>, registry: &PluginRegistry) -> Self {
        let ids = match fleet_ids {
            None => {
                return Self {
                    outcome: NativeStrategyBootstrapOutcome::Dormant,
                }
            }
            Some([]) => {
                return Self {
                    outcome: NativeStrategyBootstrapOutcome::Dormant,
                }
            }
            Some(ids) => ids,
        };

        // Single-strategy Tier A policy: consume only the first fleet entry.
        let strategy_id = ids[0].clone();

        match registry.instantiate_verified(&strategy_id) {
            Ok(instance) => {
                // B1C: shadow mode lifted — bar ingestion (B1B) and decision
                // submission bridge (B1C) are now wired.
                let mut host = StrategyHost::new(ShadowMode::Off);
                match host.register(instance) {
                    Ok(()) => Self {
                        outcome: NativeStrategyBootstrapOutcome::Active { host, strategy_id },
                    },
                    Err(e) => Self {
                        outcome: NativeStrategyBootstrapOutcome::Failed {
                            strategy_id,
                            reason: format!("host registration failed: {e:?}"),
                        },
                    },
                }
            }
            Err(e) => Self {
                outcome: NativeStrategyBootstrapOutcome::Failed {
                    strategy_id,
                    reason: e.to_string(),
                },
            },
        }
    }

    /// Returns `true` if the bootstrap produced an active strategy host.
    pub fn is_active(&self) -> bool {
        matches!(self.outcome, NativeStrategyBootstrapOutcome::Active { .. })
    }

    /// Returns `true` if no strategy fleet is configured (dormant; not an error).
    pub fn is_dormant(&self) -> bool {
        matches!(self.outcome, NativeStrategyBootstrapOutcome::Dormant)
    }

    /// Returns `true` if the bootstrap failed (fleet present, registry miss).
    pub fn is_failed(&self) -> bool {
        matches!(self.outcome, NativeStrategyBootstrapOutcome::Failed { .. })
    }

    /// Returns the `strategy_id` of the active strategy, or `None`.
    pub fn active_strategy_id(&self) -> Option<&str> {
        match &self.outcome {
            NativeStrategyBootstrapOutcome::Active { strategy_id, .. } => Some(strategy_id),
            _ => None,
        }
    }

    /// Returns the failure reason if the bootstrap failed, or `None`.
    pub fn failure_reason(&self) -> Option<&str> {
        match &self.outcome {
            NativeStrategyBootstrapOutcome::Failed { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// B1A truth-state string for observability and gate error messages.
    pub fn truth_state(&self) -> &'static str {
        match &self.outcome {
            NativeStrategyBootstrapOutcome::Dormant => "dormant",
            NativeStrategyBootstrapOutcome::Active { .. } => "active",
            NativeStrategyBootstrapOutcome::Failed { .. } => "failed",
        }
    }

    /// B1B: Invoke `on_bar` from an operator signal payload.
    ///
    /// Reads the registered strategy's `timeframe_secs` from its spec, builds a
    /// [`StrategyContext`] via [`build_signal_context`], and dispatches `on_bar`
    /// on the active host.
    ///
    /// Returns `Some(StrategyBarResult)` when the bootstrap is Active and the
    /// callback succeeds.  Returns `None` for Dormant, Failed, spec-read error,
    /// or timeframe mismatch — all treated as fail-closed (no callback).
    ///
    /// The result carries [`mqk_strategy::IntentMode::Live`] after B1C (shadow
    /// mode lifted; decision submission bridge wired).
    pub fn invoke_on_bar_from_signal(
        &mut self,
        now_tick: u64,
        end_ts: i64,
        limit_price: Option<i64>,
        qty: i64,
    ) -> Option<StrategyBarResult> {
        match &mut self.outcome {
            NativeStrategyBootstrapOutcome::Active { host, .. } => {
                let timeframe_secs = host.spec().ok()?.timeframe_secs;
                let ctx =
                    build_signal_context(timeframe_secs, now_tick, end_ts, limit_price, qty);
                host.on_bar(&ctx).ok()
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// B1B: Signal context builder
// ---------------------------------------------------------------------------

/// Build a [`StrategyContext`] from an operator signal payload.
///
/// B1B/B1C canonical input bridge.  A single [`BarStub`] is constructed from the
/// signal's price and size fields:
///
/// - `end_ts` — Unix timestamp (seconds) of the bar close; use daemon
///   session clock (`session_now_ts`).
/// - `is_complete` — `limit_price.is_some()`. Market orders carry no price
///   reference; the bar is marked incomplete. Strategies that
///   gate on `bar.is_complete` will return no targets — correct
///   conservative behavior, not a silent error.
/// - `close_micros` — `limit_price.unwrap_or(0)` (price in micros).
/// - `volume`       — `qty` (integer share count from signal).
///
/// Strategies requiring multi-bar lookback (e.g. `swing_momentum` with
/// `LOOKBACK=20`) will return empty targets because the window has exactly one
/// bar.  This is honest: there is no fabricated historical context.
///
/// The function is pure (no IO, no global state) and exported for test isolation.
pub fn build_signal_context(
    timeframe_secs: i64,
    now_tick: u64,
    end_ts: i64,
    limit_price: Option<i64>,
    qty: i64,
) -> StrategyContext {
    let bar = BarStub::new(
        end_ts,
        limit_price.is_some(),
        limit_price.unwrap_or(0),
        qty,
    );
    let recent = RecentBarsWindow::new(1, vec![bar]);
    StrategyContext::new(timeframe_secs, now_tick, recent)
}

// ---------------------------------------------------------------------------
// Daemon plugin registry constructor
// ---------------------------------------------------------------------------

/// Build the daemon's native strategy plugin registry.
///
/// Registers all four built-in strategy engines (swing_momentum, mean_reversion,
/// volatility_breakout, intraday_scalper).  The trading symbol for each factory
/// is read from `MQK_STRATEGY_SYMBOL`; if absent the empty string is used as a
/// placeholder.  The symbol is captured in factory closures but is not consumed
/// during B1A because bar ingestion (`on_bar`) is not yet wired — strategies run
/// in shadow mode only.
///
/// Operators may now configure `MQK_STRATEGY_IDS` with any of the four built-in
/// engine names.  Unknown names still produce a fail-closed start refusal via
/// the native strategy bootstrap gate.
pub fn build_daemon_plugin_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    // Symbol captured in closures; provided to strategy engines via on_bar context.
    let symbol = std::env::var("MQK_STRATEGY_SYMBOL").unwrap_or_default();
    mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol)
        .expect("daemon built-in strategy registration must not fail: duplicate names are a programming error");
    registry
}
