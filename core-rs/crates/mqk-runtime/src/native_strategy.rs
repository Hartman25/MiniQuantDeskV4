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
    BarStub, PluginRegistry, RecentBarsWindow, ShadowMode, SizingPolicy, StrategyBarResult,
    StrategyContext, StrategyHost, TargetSizing, SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
    SIZING_POLICY_FIXED_QUANTITY_V1,
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
///
/// A sizing refusal (e.g. Crypto without an explicit exact size) returns an
/// EMPTY registry: no strategy can be instantiated. Callers wanting the
/// refusal reason use [`build_plugin_registry_from_inputs`].
pub fn build_daemon_plugin_registry_and_symbol() -> (PluginRegistry, Option<String>) {
    let inputs = StrategyBootstrapInputs::from_process_env();
    let trimmed = inputs.symbol.trim();
    let effective_symbol = (!trimmed.is_empty()).then(|| trimmed.to_string());
    let registry = build_plugin_registry_from_inputs(&inputs).unwrap_or_default();
    (registry, effective_symbol)
}

/// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 5: build a plugin registry for
/// one explicit, caller-supplied symbol — never `MQK_STRATEGY_SYMBOL`. The
/// symbol source is a parameter; sizing/asset-class resolution is identical
/// to [`build_daemon_plugin_registry_and_symbol`] (registry-v2 truth; a
/// refusal yields an empty registry). Bundle 7's per-symbol run-scoped host
/// pool uses this to instantiate one strategy for one exact selected symbol.
pub fn build_daemon_plugin_registry_for_symbol(symbol: &str) -> PluginRegistry {
    try_build_daemon_plugin_registry_for_symbol(symbol).unwrap_or_default()
}

/// [`build_daemon_plugin_registry_for_symbol`] that surfaces the refusal.
pub fn try_build_daemon_plugin_registry_for_symbol(
    symbol: &str,
) -> Result<PluginRegistry, StrategySizingResolutionError> {
    build_plugin_registry_from_inputs(&StrategyBootstrapInputs::from_process_env_for_symbol(
        symbol,
    ))
}

/// Env var naming the registry-v2 trading authority (same source the daemon's
/// order-admission seam resolves the instrument asset class from).
pub const TRADING_REGISTRY_V2_PATH_ENV: &str = "MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH";

/// Deployment sizing-contract inputs. Unset = the historical fixed-quantity
/// contract; the capital-fraction contract has no defaults.
pub const SIZING_POLICY_ENV: &str = "MQK_STRATEGY_SIZING_POLICY";
pub const ALLOCATION_FRACTION_BPS_ENV: &str = "MQK_STRATEGY_ALLOCATION_FRACTION_BPS";
pub const ALLOCATED_CAPITAL_MICROS_ENV: &str = "MQK_STRATEGY_ALLOCATED_CAPITAL_MICROS";

/// Raw operator inputs a production strategy registry is built from, kept as
/// unparsed strings so sizing is resolved exactly once, by
/// [`resolve_strategy_target_sizing`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategyBootstrapInputs {
    pub symbol: String,
    pub trading_registry_v2_path: Option<String>,
    pub raw_target_qty: Option<String>,
    pub raw_max_target_qty: Option<String>,
    pub raw_max_notional_usd: Option<String>,
    pub raw_sizing_policy: Option<String>,
    pub raw_allocation_fraction_bps: Option<String>,
    pub raw_allocated_capital_micros: Option<String>,
}

impl StrategyBootstrapInputs {
    /// Read every input from the process environment for `symbol`.
    pub fn from_process_env_for_symbol(symbol: impl Into<String>) -> Self {
        use mqk_strategy::engines::intraday_scalper as scalper;
        let env = |name: &str| std::env::var(name).ok();
        Self {
            symbol: symbol.into(),
            trading_registry_v2_path: env(TRADING_REGISTRY_V2_PATH_ENV),
            raw_target_qty: env(scalper::TARGET_QTY_ENV),
            raw_max_target_qty: env(scalper::MAX_TARGET_QTY_ENV),
            raw_max_notional_usd: env(scalper::MAX_NOTIONAL_USD_ENV),
            raw_sizing_policy: env(SIZING_POLICY_ENV),
            raw_allocation_fraction_bps: env(ALLOCATION_FRACTION_BPS_ENV),
            raw_allocated_capital_micros: env(ALLOCATED_CAPITAL_MICROS_ENV),
        }
    }

    /// Symbol from `MQK_STRATEGY_SYMBOL` (empty placeholder when unset).
    pub fn from_process_env() -> Self {
        Self::from_process_env_for_symbol(std::env::var("MQK_STRATEGY_SYMBOL").unwrap_or_default())
    }
}

/// Fail-closed refusal to resolve production strategy sizing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategySizingResolutionError(pub String);

impl std::fmt::Display for StrategySizingResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StrategySizingResolutionError {}

/// Resolve the exact target sizing a production strategy instance is built with.
///
/// Asset class comes from registry-v2 truth for `inputs.symbol`, never from
/// symbol spelling:
/// - no registry-v2 path configured, blank symbol, or symbol absent from the
///   registry => Equity (historical env semantics; the daemon's admission seam
///   independently proves legacy-Equity authority per decision);
/// - `equity` row => Equity;
/// - `crypto` row => explicit exact size mandatory ([`TargetSizing::resolve`]),
///   validated (never chosen) against the row's increment / minimum economics;
/// - any other class, or an unreadable/invalid registry => refused.
pub fn resolve_strategy_target_sizing(
    inputs: &StrategyBootstrapInputs,
) -> Result<TargetSizing, StrategySizingResolutionError> {
    use mqk_execution::AssetClass;
    use mqk_md::instrument_registry_v2 as v2;
    let refuse = StrategySizingResolutionError;

    let symbol = inputs.symbol.trim();
    let resolve_as = |class: AssetClass| {
        TargetSizing::resolve(
            class,
            inputs.raw_target_qty.as_deref(),
            inputs.raw_max_target_qty.as_deref(),
            inputs.raw_max_notional_usd.as_deref(),
        )
    };
    let resolve_equity = || {
        resolve_as(AssetClass::Equity)
            .map_err(|e| refuse(format!("equity strategy sizing refused: {e}")))
    };

    let path = inputs
        .trading_registry_v2_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let Some(path) = path.filter(|_| !symbol.is_empty()) else {
        return resolve_equity();
    };

    let registry = v2::load_instrument_registry_v2(std::path::Path::new(path)).map_err(|e| {
        refuse(format!(
            "trading registry-v2 load failed from '{path}': {e}"
        ))
    })?;
    v2::validate_registry_v2(&registry)
        .map_err(|e| refuse(format!("trading registry-v2 validation failed: {e}")))?;
    if registry
        .instruments
        .iter()
        .any(|i| i.allow_enabled_non_equity_for_testing)
    {
        return Err(refuse(
            "trading registry-v2 carries allow_enabled_non_equity_for_testing=true; \
             test-only bypasses are forbidden as production sizing authority"
                .to_string(),
        ));
    }

    let Some(instrument) = registry
        .instruments
        .iter()
        .find(|i| i.symbol.trim() == symbol)
    else {
        return resolve_equity();
    };

    match v2::registry_v2_gate_asset_class(&instrument.asset_class)
        .map_err(|e| refuse(format!("strategy asset class for '{symbol}' refused: {e}")))?
    {
        v2::RegistryV2GateAssetClass::Equity => resolve_equity(),
        v2::RegistryV2GateAssetClass::NonEquity { asset_class } if asset_class == "crypto" => {
            let sizing = resolve_as(AssetClass::Crypto)
                .map_err(|e| refuse(format!("crypto strategy '{symbol}' sizing refused: {e}")))?;
            let economics = instrument.economics.as_ref();
            let (Some(increment), Some(min_trade)) = (
                economics.and_then(|e| e.quantity_increment_micros),
                economics.and_then(|e| e.min_trade_qty_micros),
            ) else {
                return Err(refuse(format!(
                    "crypto strategy '{symbol}' refused: registry-v2 economics lack \
                     quantity_increment_micros/min_trade_qty_micros"
                )));
            };
            sizing.validate_against(increment, min_trade).map_err(|e| {
                refuse(format!(
                    "crypto strategy '{symbol}' explicit size violates registry-v2 economics: {e}"
                ))
            })?;
            Ok(sizing)
        }
        v2::RegistryV2GateAssetClass::NonEquity { asset_class } => Err(refuse(format!(
            "strategy '{symbol}' asset class '{asset_class}' has no supported target-sizing policy"
        ))),
    }
}

/// Validated deployment sizing contract: the versioned policy plus, for the
/// capital-fraction policy, the explicit immutable allocated capital.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeploymentSizingContract {
    pub policy: SizingPolicy,
    pub allocated_capital_micros: Option<i64>,
}

impl DeploymentSizingContract {
    /// Deterministic identity token; EMPTY for the historical fixed-quantity
    /// contract so legacy deployment identity is unchanged.
    pub fn identity_token(&self) -> String {
        match (
            self.policy.allocation_fraction_bps(),
            self.allocated_capital_micros,
        ) {
            (Some(bps), Some(cap)) => format!(
                "sz_policy={}|sz_frac_bps={bps}|sz_capital_micros={cap}",
                self.policy.policy_id()
            ),
            _ => String::new(),
        }
    }
}

/// Strictly resolve the deployment sizing contract. Nothing set => legacy
/// fixed quantity. Partial, mixed, unknown, malformed or out-of-range inputs
/// are refused; there is no default fraction, capital, or one-share fallback,
/// and nothing is inferred from a broker balance.
pub fn resolve_deployment_sizing_contract(
    inputs: &StrategyBootstrapInputs,
) -> Result<DeploymentSizingContract, StrategySizingResolutionError> {
    let refuse = |m: String| StrategySizingResolutionError(format!("sizing contract refused: {m}"));
    fn trim(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim)
    }
    let (policy, bps, cap) = (
        trim(&inputs.raw_sizing_policy),
        trim(&inputs.raw_allocation_fraction_bps),
        trim(&inputs.raw_allocated_capital_micros),
    );
    let strict_int = |name: &str, raw: &str| -> Result<i64, StrategySizingResolutionError> {
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse(format!("{name} must be a plain positive integer")));
        }
        raw.parse::<i64>()
            .map_err(|_| refuse(format!("{name} is out of range")))
    };
    match policy {
        None if bps.is_none() && cap.is_none() => Ok(DeploymentSizingContract {
            policy: SizingPolicy::FixedQuantityV1,
            allocated_capital_micros: None,
        }),
        None => Err(refuse(
            "capital-fraction fields are set without an explicit sizing policy".to_string(),
        )),
        Some(SIZING_POLICY_FIXED_QUANTITY_V1) if bps.is_none() && cap.is_none() => {
            Ok(DeploymentSizingContract {
                policy: SizingPolicy::FixedQuantityV1,
                allocated_capital_micros: None,
            })
        }
        Some(SIZING_POLICY_FIXED_QUANTITY_V1) => Err(refuse(
            "fixed_quantity_v1 cannot carry capital-fraction fields".to_string(),
        )),
        Some(SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1) => {
            let (Some(bps), Some(cap)) = (bps, cap) else {
                return Err(refuse(
                    "capital-fraction policy requires explicit allocation_fraction_bps and \
                     allocated_capital_micros"
                        .to_string(),
                ));
            };
            let policy =
                SizingPolicy::capital_fraction_v1(strict_int("allocation_fraction_bps", bps)?)
                    .map_err(|e| refuse(e.reason_code().to_string()))?;
            let cap = strict_int("allocated_capital_micros", cap)?;
            if cap <= 0 {
                return Err(refuse(
                    "allocated_capital_micros must be positive".to_string(),
                ));
            }
            Ok(DeploymentSizingContract {
                policy,
                allocated_capital_micros: Some(cap),
            })
        }
        Some(other) => Err(refuse(format!("unknown sizing policy '{other}'"))),
    }
}

/// The validated capital-fraction deployment contract: the policy, the
/// immutable initial allocated capital, and the caps (strictly parsed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapitalFractionDeploymentContract {
    pub policy: SizingPolicy,
    pub allocation_fraction_bps: i64,
    pub allocated_capital_micros: i64,
    pub caps: TargetSizing,
}

/// Resolve the capital-fraction contract from `inputs`. `Ok(None)` is the
/// historical fixed-quantity contract. Caps are parsed STRICTLY here (a
/// malformed cap is refused, never silently dropped as in the fixed-quantity
/// env semantics), and only Equity is supported: asset class comes from
/// registry-v2 truth, never from the symbol.
pub fn resolve_capital_fraction_deployment(
    inputs: &StrategyBootstrapInputs,
) -> Result<Option<CapitalFractionDeploymentContract>, StrategySizingResolutionError> {
    let contract = resolve_deployment_sizing_contract(inputs)?;
    let (Some(bps), Some(capital)) = (
        contract.policy.allocation_fraction_bps(),
        contract.allocated_capital_micros,
    ) else {
        return Ok(None);
    };
    let refuse = |m: String| StrategySizingResolutionError(format!("sizing contract refused: {m}"));
    // Asset class authority: the registry-v2 class of the symbol must be
    // Equity. Resolve with no size/caps so only the class can decide.
    let class_probe = StrategyBootstrapInputs {
        raw_target_qty: None,
        raw_max_target_qty: None,
        raw_max_notional_usd: None,
        ..inputs.clone()
    };
    let probe = resolve_strategy_target_sizing(&class_probe)?;
    if probe.asset_class() != mqk_execution::AssetClass::Equity {
        return Err(refuse(
            "capital-fraction sizing supports Equity only".to_string(),
        ));
    }
    let strict_cap =
        |name: &str, raw: &Option<String>| -> Result<Option<i64>, StrategySizingResolutionError> {
            match raw.as_deref().map(str::trim) {
                None => Ok(None),
                Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                    match v.parse::<i64>() {
                        Ok(n) if n > 0 => Ok(Some(n)),
                        _ => Err(refuse(format!("{name} must be a positive integer"))),
                    }
                }
                Some(_) => Err(refuse(format!("{name} must be a positive integer"))),
            }
        };
    let max_qty = strict_cap("max_target_qty", &inputs.raw_max_target_qty)?;
    let max_notional = strict_cap("max_notional_usd", &inputs.raw_max_notional_usd)?;
    // `target` is the fixed-quantity size; the capital-fraction wrapper
    // ignores it, so it is pinned to the historical 1 and a supplied value is
    // refused rather than ignored.
    if inputs
        .raw_target_qty
        .as_deref()
        .map(str::trim)
        .is_some_and(|v| !v.is_empty())
    {
        return Err(refuse(
            "target_qty is the fixed-quantity size and is not accepted with the capital-fraction policy"
                .to_string(),
        ));
    }
    let caps = TargetSizing::equity_whole_units(1, max_qty, max_notional)
        .map_err(|e| refuse(format!("invalid caps: {e}")))?;
    Ok(Some(CapitalFractionDeploymentContract {
        policy: contract.policy,
        allocation_fraction_bps: bps,
        allocated_capital_micros: capital,
        caps,
    }))
}

/// The resolved semantic identity of one native strategy under the deployment
/// contract: the unwrapped engine fingerprint for the fixed-quantity contract,
/// the capital-fraction WRAPPER fingerprint (the one Backtest and Promotion
/// use; derived by the same pure function) otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDeploymentIdentity {
    pub semantic_fingerprint: String,
    pub timeframe_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeIdentityError {
    Sizing(StrategySizingResolutionError),
    /// Unknown strategy, restart-unrecoverable engine, or registry
    /// metadata/spec inconsistency.
    UnsupportedStrategy,
}

pub fn resolve_native_deployment_identity(
    inputs: &StrategyBootstrapInputs,
    strategy_id: &str,
) -> Result<NativeDeploymentIdentity, NativeIdentityError> {
    let registry =
        build_plugin_registry_from_inputs(inputs).map_err(NativeIdentityError::Sizing)?;
    let cf = resolve_capital_fraction_deployment(inputs).map_err(NativeIdentityError::Sizing)?;
    let (inner, _) = registry
        .instantiate_for_identity(strategy_id)
        .map_err(|_| NativeIdentityError::UnsupportedStrategy)?;
    let timeframe_secs = inner.spec().timeframe_secs;
    let inner_fingerprint = inner.semantic_fingerprint();
    let semantic_fingerprint = match cf {
        None => inner_fingerprint,
        Some(c) => mqk_strategy::capital_fraction_semantic_fingerprint(
            &inner_fingerprint,
            c.allocation_fraction_bps,
            c.allocated_capital_micros,
            &c.caps,
        ),
    };
    Ok(NativeDeploymentIdentity {
        semantic_fingerprint,
        timeframe_secs,
    })
}

/// Build the production plugin registry from explicit inputs. Every asset
/// class goes through resolved [`TargetSizing`]; a refusal yields no registry
/// at all (fail closed).
///
/// A capital-fraction contract yields a registry whose entries are the
/// UNWRAPPED engines marked `DurableStateRequired`: the stateless seams
/// (`instantiate_verified`: bootstrap, host pool, promotion identity) refuse
/// them, so nothing can run one without a runtime that recovers and persists
/// the held quantity (`capital_fraction_host`). Identity resolves through
/// [`resolve_native_deployment_identity`]. The inner engines are registered
/// exactly as Backtest registers them (default one-share sizing; the wrapper
/// owns quantity and caps).
pub fn build_plugin_registry_from_inputs(
    inputs: &StrategyBootstrapInputs,
) -> Result<PluginRegistry, StrategySizingResolutionError> {
    if resolve_capital_fraction_deployment(inputs)?.is_some() {
        let mut registry = PluginRegistry::new();
        mqk_strategy::engines::register_builtin_strategies_with_sizing(
            &mut registry,
            inputs.symbol.clone(),
            1,
            None,
            None,
        )
        .map_err(|e| {
            StrategySizingResolutionError(format!("built-in strategy registration failed: {e}"))
        })?;
        return Ok(registry.with_durable_state_required());
    }
    let sizing = resolve_strategy_target_sizing(inputs)?;
    let mut registry = PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies_with_target_sizing(
        &mut registry,
        inputs.symbol.clone(),
        sizing,
    )
    .map_err(|e| {
        StrategySizingResolutionError(format!("built-in strategy registration failed: {e}"))
    })?;
    Ok(registry)
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
    bootstrap_with_effective_binding_from_inputs(
        fleet_ids,
        &StrategyBootstrapInputs::from_process_env(),
    )
}

/// [`bootstrap_with_effective_binding`] over explicit inputs (the production
/// function body; the env variant only supplies the inputs).
///
/// A sizing refusal is `Failed` (with the refusal reason) when a fleet
/// strategy is selected, and irrelevant (`Dormant`) when none is.
pub fn bootstrap_with_effective_binding_from_inputs(
    fleet_ids: Option<&[String]>,
    inputs: &StrategyBootstrapInputs,
) -> (NativeStrategyBootstrap, EffectiveRuntimeBinding) {
    let trimmed = inputs.symbol.trim();
    let effective_runtime_target_symbol = (!trimmed.is_empty()).then(|| trimmed.to_string());
    let bootstrap = match build_plugin_registry_from_inputs(inputs) {
        Ok(registry) => NativeStrategyBootstrap::bootstrap(fleet_ids, &registry),
        Err(refusal) => match fleet_ids.and_then(|ids| ids.first()) {
            Some(strategy_id) => NativeStrategyBootstrap {
                outcome: NativeStrategyBootstrapOutcome::Failed {
                    strategy_id: strategy_id.clone(),
                    reason: refusal.to_string(),
                },
            },
            None => NativeStrategyBootstrap {
                outcome: NativeStrategyBootstrapOutcome::Dormant,
            },
        },
    };
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
        let qty: i64 = result
            .intents
            .output
            .targets
            .iter()
            .map(|t| t.qty.to_whole_units_checked().unwrap())
            .sum();
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
        let qty: i64 = result
            .intents
            .output
            .targets
            .iter()
            .map(|t| t.qty.to_whole_units_checked().unwrap())
            .sum();
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
        let qty: i64 = result
            .intents
            .output
            .targets
            .iter()
            .map(|t| t.qty.to_whole_units_checked().unwrap())
            .sum();
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

    // ── DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 5 ──────────────────────

    #[test]
    fn build_daemon_plugin_registry_for_symbol_registers_all_builtins() {
        let registry = build_daemon_plugin_registry_for_symbol("AAPL");
        let names: Vec<&str> = registry.list().iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"swing_momentum"));
        assert!(names.contains(&"mean_reversion"));
        assert!(names.contains(&"volatility_breakout"));
        assert!(names.contains(&"intraday_scalper"));
        assert!(names.contains(&"trend_sma50"));
        assert!(names.contains(&"dual_sma_50_200_trend"));
        assert!(names.contains(&"pullback_mean_reversion_20_2"));
        assert!(names.contains(&"absolute_momentum_252"));
        assert!(names.contains(&"near_high_momentum_252_3pct"));
        assert!(names.contains(&"trend_pullback_5d_4pct_hold5"));
        assert!(names.contains(&"turn_of_month_last1_first3"));
        assert!(names.contains(&"halloween_nov_apr"));
        assert!(names.contains(&"trading_range_breakout_50d_hold10"));
        assert!(names.contains(&"monthly_multihorizon_abs_momentum_consensus_v1"));
        assert!(names.contains(&"trend_filtered_rsi5_reversion_v1"));
        assert!(names.contains(&"trend_filtered_extreme_3d_atr_reversal_v1"));
        assert!(names.contains(&"close_channel_100_50_trend_v1"));
        assert!(names.contains(&"monthly_10month_trend_timing_v1"));
        assert!(names.contains(&"trend_filtered_zscore20_reversion_v1"));
        assert!(names.contains(&"volatility_contraction_breakout_v1"));
        assert!(names.contains(&"monthly_12_minus_1_abs_momentum_v1"));
        assert!(names.contains(&"delayed_overnight_gap_reversal_v1"));
        assert!(names.contains(&"monthly_52week_high_proximity_v1"));
    }

    /// IR-2: a restart-unsafe stateful engine can never become an Active Paper host.
    #[test]
    fn bootstrap_fails_closed_for_a_restart_unsafe_strategy() {
        let registry = build_daemon_plugin_registry_for_symbol("AAPL");
        let boot = NativeStrategyBootstrap::bootstrap(
            Some(&["pullback_mean_reversion_20_2".to_string()]),
            &registry,
        );
        assert!(boot.is_failed());
        assert!(!boot.is_active());
        assert!(boot.failure_reason().unwrap().contains("restart"));
    }

    #[test]
    fn build_daemon_plugin_registry_for_symbol_ignores_env_var() {
        // The narrow constructor must never read MQK_STRATEGY_SYMBOL -- prove
        // it by setting the env var to a different value than the symbol
        // parameter and confirming instantiate_verified still succeeds
        // (i.e. the strategy is genuinely bound to the parameter, not env).
        std::env::set_var("MQK_STRATEGY_SYMBOL", "MSFT");
        let registry = build_daemon_plugin_registry_for_symbol("AAPL");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        assert!(registry.instantiate_verified("swing_momentum").is_ok());
    }

    #[test]
    fn build_daemon_plugin_registry_for_symbol_two_symbols_are_independent() {
        let aapl_registry = build_daemon_plugin_registry_for_symbol("AAPL");
        let msft_registry = build_daemon_plugin_registry_for_symbol("MSFT");
        assert!(aapl_registry.instantiate_verified("swing_momentum").is_ok());
        assert!(msft_registry.instantiate_verified("swing_momentum").is_ok());
        // Each registry is its own instance -- registering into one never
        // affects the other's catalogue.
        assert_eq!(aapl_registry.len(), msft_registry.len());
    }
}
