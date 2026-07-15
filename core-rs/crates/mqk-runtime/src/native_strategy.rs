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

use mqk_strategy::{
    BarStub, PluginRegistry, RecentBarsWindow, ShadowMode, StrategyBarResult, StrategyContext,
    StrategyHost,
};

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
    /// B1C: the host boots with [`ShadowMode::Off`] so the execution loop can
    /// submit live decisions through the canonical 7-gate seam.
    Active {
        host: StrategyHost,
        strategy_id: String,
    },

    /// A fleet entry is present but the named strategy is not registered in
    /// the plugin registry. Fail-closed: the daemon must not start with an
    /// unresolvable strategy configuration.
    Failed { strategy_id: String, reason: String },
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

    /// AUTON-SIGNAL-CONTEXT-01: Return the registered strategy's `timeframe_secs`.
    ///
    /// Returns `None` when the bootstrap is not Active (Dormant or Failed).
    /// Used by the daemon dispatch path to derive the correct `StrategyContext`
    /// when building a window from DB bars.
    pub fn strategy_timeframe_secs(&self) -> Option<i64> {
        match &self.outcome {
            NativeStrategyBootstrapOutcome::Active { host, .. } => {
                host.spec().ok().map(|s| s.timeframe_secs)
            }
            _ => None,
        }
    }

    /// AUTON-SIGNAL-CONTEXT-01: Invoke `on_bar` with a pre-built `RecentBarsWindow`.
    ///
    /// Used when the daemon has loaded real completed bars from `md_bars` rather
    /// than creating a single-bar stub from an operator signal.  The context
    /// `timeframe_secs` is derived from the strategy's own spec so the
    /// `StrategyHost` timeframe check always passes.
    ///
    /// Returns `Some(StrategyBarResult)` when Active and the callback succeeds.
    /// Returns `None` for Dormant, Failed, or spec-read error — all fail-closed.
    pub fn invoke_on_bar_from_window(
        &mut self,
        now_tick: u64,
        recent: RecentBarsWindow,
    ) -> Option<StrategyBarResult> {
        match &mut self.outcome {
            NativeStrategyBootstrapOutcome::Active { host, .. } => {
                let timeframe_secs = host.spec().ok()?.timeframe_secs;
                let ctx = StrategyContext::new(timeframe_secs, now_tick, recent);
                host.on_bar(&ctx).ok()
            }
            _ => None,
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
                let ctx = build_signal_context(timeframe_secs, now_tick, end_ts, limit_price, qty);
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
    let bar = BarStub::new(end_ts, limit_price.is_some(), limit_price.unwrap_or(0), qty);
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
    build_daemon_plugin_registry_and_symbol().0
}

/// Identical construction to [`build_daemon_plugin_registry`], additionally
/// returning the trimmed `MQK_STRATEGY_SYMBOL` value read to build it
/// (`None` when unset/blank) — the single env read both the registry and any
/// [`EffectiveRuntimeBinding`] derived from the same bootstrap attempt must
/// share, so a caller building both never risks a second, independently-read
/// symbol value disagreeing with the one baked into the registry's strategy
/// factories (DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED Phase C).
pub fn build_daemon_plugin_registry_and_symbol() -> (PluginRegistry, Option<String>) {
    let mut registry = PluginRegistry::new();
    // Symbol captured in closures; provided to strategy engines via on_bar context.
    let symbol = std::env::var("MQK_STRATEGY_SYMBOL").unwrap_or_default();
    mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol.clone())
        .expect("daemon built-in strategy registration must not fail: duplicate names are a programming error");
    let trimmed = symbol.trim();
    let effective_symbol = (!trimmed.is_empty()).then(|| trimmed.to_string());
    (registry, effective_symbol)
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED Phase B: immutable effective
// runtime binding snapshot.
// ---------------------------------------------------------------------------

/// Immutable snapshot of what the active native-strategy bootstrap actually
/// resolved to, captured at the same moment the bootstrap itself is
/// constructed — never re-derived later by re-reading mutable environment
/// state. `None` fields mean the bootstrap is not `Active` (or, for
/// `effective_runtime_target_symbol`, that `MQK_STRATEGY_SYMBOL` was unset or
/// blank) — callers must treat `None` as "no bootstrap target to bind to",
/// not as a wildcard match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveRuntimeBinding {
    pub effective_runtime_strategy_id: Option<String>,
    pub effective_runtime_target_symbol: Option<String>,
    pub effective_runtime_timeframe_secs: Option<i64>,
}

/// Bootstrap a native strategy host (identical construction to
/// [`build_daemon_plugin_registry`] + [`NativeStrategyBootstrap::bootstrap`])
/// and capture the [`EffectiveRuntimeBinding`] snapshot from the exact same
/// `MQK_STRATEGY_SYMBOL` read and resulting bootstrap outcome — one read, one
/// derivation, so the binding can never disagree with what the bootstrap
/// itself resolved to.
///
/// `effective_runtime_target_symbol` is trimmed; empty-after-trim is reported
/// as `None` (no bootstrap target to bind to), matching the fail-closed
/// convention `retain_targets_matching_symbol` (`mqk-daemon`) already uses
/// for symbol comparison.
pub fn bootstrap_with_effective_binding(
    fleet_ids: Option<&[String]>,
) -> (NativeStrategyBootstrap, EffectiveRuntimeBinding) {
    let (registry, effective_runtime_target_symbol) = build_daemon_plugin_registry_and_symbol();
    let bootstrap = NativeStrategyBootstrap::bootstrap(fleet_ids, &registry);
    let binding = effective_binding_from_bootstrap(&bootstrap, effective_runtime_target_symbol);
    (bootstrap, binding)
}

/// Derive an [`EffectiveRuntimeBinding`] from an already-constructed
/// `bootstrap` and the `effective_runtime_target_symbol` captured from the
/// same [`build_daemon_plugin_registry_and_symbol`] call that produced the
/// registry `bootstrap` was built from.
///
/// Exists so a caller that must not construct a second bootstrap (e.g. the
/// runtime start gate, which already builds its own bootstrap via B1A) can
/// still derive the binding from the exact bootstrap it holds, instead of
/// calling [`bootstrap_with_effective_binding`] again and discarding a
/// second, independently-constructed bootstrap.
pub fn effective_binding_from_bootstrap(
    bootstrap: &NativeStrategyBootstrap,
    effective_runtime_target_symbol: Option<String>,
) -> EffectiveRuntimeBinding {
    EffectiveRuntimeBinding {
        effective_runtime_strategy_id: bootstrap.active_strategy_id().map(|s| s.to_string()),
        effective_runtime_target_symbol,
        effective_runtime_timeframe_secs: bootstrap.strategy_timeframe_secs(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scalper_bootstrap() -> NativeStrategyBootstrap {
        let mut registry = PluginRegistry::new();
        mqk_strategy::engines::register_builtin_strategies(&mut registry, "AAPL")
            .expect("registration must not fail");
        NativeStrategyBootstrap::bootstrap(Some(&["intraday_scalper".to_string()]), &registry)
    }

    fn complete_bar(end_ts: i64, close_micros: i64) -> BarStub {
        BarStub::new(end_ts, true, close_micros, 100)
    }

    /// SC-NS-01: invoke_on_bar_from_window with Dormant bootstrap returns None.
    #[test]
    fn sc_ns01_dormant_bootstrap_returns_none_for_window() {
        let registry = PluginRegistry::new();
        let mut bootstrap = NativeStrategyBootstrap::bootstrap(None, &registry);
        let window = RecentBarsWindow::new(1, vec![complete_bar(1_000_000, 200_000_000)]);
        let result = bootstrap.invoke_on_bar_from_window(0, window);
        assert!(
            result.is_none(),
            "SC-NS-01: Dormant bootstrap must return None for window dispatch"
        );
    }

    /// SC-NS-02: invoke_on_bar_from_window with 1 bar (< LOOKBACK=5) returns
    /// signal_qty=0 — insufficient lookback, not a crash.
    #[test]
    fn sc_ns02_insufficient_lookback_returns_zero_signal() {
        let mut bootstrap = make_scalper_bootstrap();
        // Only 1 complete bar — intraday_scalper needs 5.
        let window = RecentBarsWindow::new(1, vec![complete_bar(1_000_000, 200_000_000)]);
        let result = bootstrap
            .invoke_on_bar_from_window(0, window)
            .expect("SC-NS-02: Active bootstrap must return Some");
        let qty: i64 = result.intents.output.targets.iter().map(|t| t.qty).sum();
        assert_eq!(
            qty, 0,
            "SC-NS-02: insufficient lookback → signal_qty must be 0"
        );
    }

    /// SC-NS-03: invoke_on_bar_from_window with 5 complete bars and sufficient
    /// price displacement produces a non-zero signal (proves DB window path works).
    #[test]
    fn sc_ns03_five_complete_bars_with_displacement_produces_signal() {
        let mut bootstrap = make_scalper_bootstrap();
        // intraday_scalper LOOKBACK=5, MICRO_MOVE_BPS=20 (0.20%)
        // Use price rising from 100 USD to 100.25 USD (25 bps > 20 bps threshold).
        let bars = vec![
            complete_bar(1_000_000, 100_000_000), // 100.000000 USD
            complete_bar(1_000_300, 100_050_000), // 100.050000 USD
            complete_bar(1_000_600, 100_100_000), // 100.100000 USD
            complete_bar(1_000_900, 100_150_000), // 100.150000 USD
            complete_bar(1_001_200, 100_250_000), // 100.250000 USD — 25 bps above bar[0]
        ];
        let window = RecentBarsWindow::new(5, bars);
        let result = bootstrap
            .invoke_on_bar_from_window(1, window)
            .expect("SC-NS-03: Active bootstrap must return Some");
        let qty: i64 = result.intents.output.targets.iter().map(|t| t.qty).sum();
        assert_eq!(
            qty, 1,
            "SC-NS-03: 5 complete bars with 25 bps displacement must produce signal_qty=1"
        );
    }

    /// SC-NS-04: invoke_on_bar_from_window with incomplete last bar returns 0.
    #[test]
    fn sc_ns04_incomplete_last_bar_returns_zero_signal() {
        let mut bootstrap = make_scalper_bootstrap();
        let mut bars = vec![
            complete_bar(1_000_000, 100_000_000),
            complete_bar(1_000_300, 100_050_000),
            complete_bar(1_000_600, 100_100_000),
            complete_bar(1_000_900, 100_150_000),
        ];
        // Last bar is incomplete — no price reference.
        bars.push(BarStub::new(1_001_200, false, 100_250_000, 100));
        let window = RecentBarsWindow::new(5, bars);
        let result = bootstrap
            .invoke_on_bar_from_window(2, window)
            .expect("SC-NS-04: Active bootstrap must return Some");
        let qty: i64 = result.intents.output.targets.iter().map(|t| t.qty).sum();
        assert_eq!(
            qty, 0,
            "SC-NS-04: incomplete last bar → signal_qty must be 0"
        );
    }

    /// SC-NS-05: strategy_timeframe_secs returns the strategy's canonical value.
    #[test]
    fn sc_ns05_timeframe_secs_matches_strategy_spec() {
        let bootstrap = make_scalper_bootstrap();
        assert_eq!(
            bootstrap.strategy_timeframe_secs(),
            Some(300),
            "SC-NS-05: intraday_scalper timeframe_secs must be 300"
        );
    }

    /// SC-NS-06: build_signal_context with no limit_price → is_complete=false.
    ///
    /// Proves the stub path still correctly marks bars incomplete when no price
    /// reference is available (B1B legacy path preservation).
    #[test]
    fn sc_ns06_build_signal_context_no_price_yields_incomplete_bar() {
        let ctx = build_signal_context(300, 0, 1_000_000, None, 1);
        let bar = ctx.recent.last().expect("SC-NS-06: must have one bar");
        assert!(
            !bar.is_complete,
            "SC-NS-06: stub bar with limit_price=None must be incomplete"
        );
        assert_eq!(
            ctx.recent.len(),
            1,
            "SC-NS-06: stub context must have exactly one bar"
        );
    }
}
