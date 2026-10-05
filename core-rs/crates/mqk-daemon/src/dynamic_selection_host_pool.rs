//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 5 — run-scoped host pool.
//!
//! One isolated, single-registration [`StrategyHost`] per selected
//! `(symbol, strategy_id, timeframe_secs)` key, built once from a plan's
//! [`selected_pairs`](mqk_portfolio::DynamicSelectionPlan::selected_pairs)
//! (paired with each candidate's own `timeframe_secs`).
//!
//! Deliberately does **not** touch [`StrategyHost::register`] or
//! [`mqk_strategy::StrategyHostError::MultiStrategyNotAllowed`] — those stay
//! exactly as Tier A defined them; this module only builds *one host per
//! key*, each host still enforcing "exactly one strategy" internally, same
//! as every other `StrategyHost` in this codebase. It also never touches
//! [`mqk_runtime::native_strategy::build_daemon_plugin_registry`] or
//! `NativeStrategyBootstrap` — the legacy env-symbol-bound single-host path
//! used by `off` mode / Tier A single-symbol dispatch is completely
//! unchanged and untouched by this module.
//!
//! # Determinism
//! Keyed by `BTreeMap<(String, String, i64), StrategyHost>` — insertion
//! order of the input slice never affects the pool's own iteration order or
//! contents (a permuted input slice with the same keys/hosts always
//! produces a pool with identical entries).
//!
//! # Isolation
//! Each key gets its own [`PluginRegistry`], built fresh via
//! [`build_daemon_plugin_registry_for_symbol`] (never the shared, `MQK_STRATEGY_SYMBOL`-bound
//! registry) — so a strategy factory closure for symbol A can never leak
//! into or be reused for symbol B. No environment variable is read per host;
//! the exact selected symbol is captured in each host's own factory
//! closures via the caller-supplied key alone.
//!
//! # Not yet wired
//! This module builds a pool in isolation and returns it — it is not yet
//! held by `AppState`, not yet constructed on run-start, and not yet
//! cleared on stop/halt/start-failure. That lifecycle wiring belongs to
//! Phase 6 (start-gate/lifecycle authority), which must decide the whole
//! activation sequence, not just where to store a field. Constructing a
//! pool here has no dispatch/economic effect of any kind: it never calls
//! `on_bar`, never touches the outbox, and never submits a decision —
//! shadow constructability (building the pool successfully) is provably
//! inert with respect to economic dispatch, by construction (this module
//! contains no dispatch call of any kind).

use std::collections::BTreeMap;

use mqk_runtime::capital_fraction_host::CapitalFractionRuntimeHost;
use mqk_runtime::native_strategy::{
    build_daemon_plugin_registry_for_symbol, resolve_capital_fraction_paper_binding,
    NativeIdentityError, StrategyBootstrapInputs,
};
use mqk_strategy::{RegistryError, ShadowMode, StrategyHost};
use sqlx::PgPool;

/// `(symbol, strategy_id, timeframe_secs)` — the exact identity triple the
/// pure selector uses, so a pool key can always be derived directly from one
/// [`mqk_portfolio::SelectionCandidateResult`] without translation.
pub type HostPoolKey = (String, String, i64);

/// Every way [`DynamicSelectionHostPool::build`] can fail. All variants are
/// fail-closed: none of them produce a partial pool — see
/// [`DynamicSelectionHostPool::build`]'s doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPoolBuildError {
    /// The same `(symbol, strategy_id, timeframe_secs)` key appeared more
    /// than once in the input — the pure selector guarantees at most one
    /// selection per symbol, so a duplicate key here indicates a caller
    /// contract violation, never silently deduplicated.
    DuplicateKey {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    /// `strategy_id` is not registered in the per-symbol plugin registry.
    UnknownStrategy { symbol: String, strategy_id: String },
    /// The registry's own internal metadata/spec timeframe consistency
    /// check (`PluginRegistry::instantiate_verified`) failed.
    RegistryInconsistent {
        symbol: String,
        strategy_id: String,
        reason: String,
    },
    /// The instantiated strategy's `spec().name` does not equal the
    /// requested `strategy_id` — checked independently of the registry's
    /// own internal consistency check, after instantiation.
    SpecNameMismatch {
        symbol: String,
        strategy_id: String,
        expected: String,
        actual: String,
    },
    /// The instantiated strategy's `spec().timeframe_secs` does not equal
    /// this key's `timeframe_secs` — checked independently of the
    /// registry's own internal consistency check, after instantiation.
    SpecTimeframeMismatch {
        symbol: String,
        strategy_id: String,
        expected: i64,
        actual: i64,
    },
    /// `StrategyHost::register` itself refused (Tier A's own
    /// `MultiStrategyNotAllowed`/etc.) — unreachable in normal operation
    /// (each host is freshly constructed and registered exactly once), kept
    /// for exhaustive fail-closed handling rather than an `.expect()`.
    HostRegistrationFailed {
        symbol: String,
        strategy_id: String,
        reason: String,
    },
    /// The capital-fraction sizing contract in the process environment was
    /// refused (partial/unknown/malformed) or the strategy cannot run under
    /// it. No default is substituted.
    CapitalFractionContractRefused {
        symbol: String,
        strategy_id: String,
        reason: String,
    },
    /// A capital-fraction deployment needs the durable held-sizing store and
    /// no database pool was supplied.
    DurableStateUnavailable { symbol: String, strategy_id: String },
    /// Durable recovery of the held sizing state failed or the recovered host
    /// does not carry the identity the deployment resolved.
    DurableRecoveryFailed {
        symbol: String,
        strategy_id: String,
        reason: String,
    },
}

impl HostPoolBuildError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DuplicateKey { .. } => "host_pool_duplicate_key",
            Self::UnknownStrategy { .. } => "host_pool_unknown_strategy",
            Self::RegistryInconsistent { .. } => "host_pool_registry_inconsistent",
            Self::SpecNameMismatch { .. } => "host_pool_spec_name_mismatch",
            Self::SpecTimeframeMismatch { .. } => "host_pool_spec_timeframe_mismatch",
            Self::HostRegistrationFailed { .. } => "host_pool_host_registration_failed",
            Self::CapitalFractionContractRefused { .. } => {
                "host_pool_capital_fraction_contract_refused"
            }
            Self::DurableStateUnavailable { .. } => "host_pool_durable_state_unavailable",
            Self::DurableRecoveryFailed { .. } => "host_pool_durable_recovery_failed",
        }
    }
}

/// The one authoritative host for a binding: the stateless fixed-quantity
/// host, or the restart-recovered capital-fraction runtime host that persists
/// held sizing before returning any result.
pub enum RuntimeSelectedStrategyHost {
    Stateless(StrategyHost),
    DurableCapitalFraction(Box<CapitalFractionRuntimeHost>),
}

impl RuntimeSelectedStrategyHost {
    pub fn semantic_fingerprint(&self) -> Result<String, String> {
        match self {
            Self::Stateless(h) => h.semantic_fingerprint().map_err(|e| format!("{e:?}")),
            Self::DurableCapitalFraction(h) => h.semantic_fingerprint().map_err(|e| e.to_string()),
        }
    }

    pub fn is_durable(&self) -> bool {
        matches!(self, Self::DurableCapitalFraction(_))
    }
}

/// One isolated host per selected `(symbol, strategy_id, timeframe_secs)`
/// key, for exactly one active run.
pub struct DynamicSelectionHostPool {
    hosts: BTreeMap<HostPoolKey, RuntimeSelectedStrategyHost>,
}

impl DynamicSelectionHostPool {
    /// Build one host per `(symbol, strategy_id, timeframe_secs)` key in
    /// `selected`. Fails closed, returning the *first* error encountered
    /// (in `selected`'s own order) and constructing no pool at all — never a
    /// partially-built pool with some keys present and others silently
    /// skipped.
    ///
    /// For each key: builds a fresh per-symbol registry
    /// ([`build_daemon_plugin_registry_for_symbol`]), instantiates
    /// `strategy_id` via `instantiate_verified` (the registry's own
    /// name/metadata-timeframe consistency check), independently verifies
    /// the instantiated strategy's `spec().name`/`spec().timeframe_secs`
    /// against the key, then registers it into a brand new
    /// `StrategyHost::new(ShadowMode::Off)`.
    pub fn build(selected: &[HostPoolKey]) -> Result<Self, HostPoolBuildError> {
        let mut hosts: BTreeMap<HostPoolKey, RuntimeSelectedStrategyHost> = BTreeMap::new();
        for (symbol, strategy_id, timeframe_secs) in selected {
            let key = Self::fresh_key(&hosts, symbol, strategy_id, *timeframe_secs)?;
            let host = build_stateless_host(symbol, strategy_id, *timeframe_secs)?;
            hosts.insert(key, RuntimeSelectedStrategyHost::Stateless(host));
        }
        Ok(Self { hosts })
    }

    /// Same contract as [`Self::build`], additionally resolving the
    /// capital-fraction deployment per key from the process environment (read
    /// once, here, at start). A fixed-quantity key builds the identical
    /// stateless host; a capital-fraction key awaits
    /// `CapitalFractionRuntimeHost::recover` exactly once, so a recovery
    /// failure fails the whole build (and therefore the start) closed.
    pub async fn build_with_durable_state(
        selected: &[HostPoolKey],
        db: Option<&PgPool>,
    ) -> Result<Self, HostPoolBuildError> {
        Self::build_with_durable_state_from(selected, db, |symbol: &str| {
            StrategyBootstrapInputs::from_process_env_for_symbol(symbol)
        })
        .await
    }

    /// [`Self::build_with_durable_state`] with an explicit per-symbol sizing
    /// input source (the process environment in production).
    pub async fn build_with_durable_state_from(
        selected: &[HostPoolKey],
        db: Option<&PgPool>,
        inputs_for_symbol: impl Fn(&str) -> StrategyBootstrapInputs,
    ) -> Result<Self, HostPoolBuildError> {
        let mut hosts: BTreeMap<HostPoolKey, RuntimeSelectedStrategyHost> = BTreeMap::new();
        for (symbol, strategy_id, timeframe_secs) in selected {
            let key = Self::fresh_key(&hosts, symbol, strategy_id, *timeframe_secs)?;
            let inputs = inputs_for_symbol(symbol);
            let binding =
                resolve_capital_fraction_paper_binding(&inputs, strategy_id).map_err(|e| {
                    HostPoolBuildError::CapitalFractionContractRefused {
                        symbol: symbol.clone(),
                        strategy_id: strategy_id.clone(),
                        reason: match e {
                            NativeIdentityError::Sizing(s) => format!("{s:?}"),
                            NativeIdentityError::UnsupportedStrategy => {
                                "strategy is unknown or not restart-recoverable".to_string()
                            }
                        },
                    }
                })?;
            let host = match binding {
                None => RuntimeSelectedStrategyHost::Stateless(build_stateless_host(
                    symbol,
                    strategy_id,
                    *timeframe_secs,
                )?),
                Some(b) => {
                    let Some(pool) = db else {
                        return Err(HostPoolBuildError::DurableStateUnavailable {
                            symbol: symbol.clone(),
                            strategy_id: strategy_id.clone(),
                        });
                    };
                    if b.timeframe_secs != *timeframe_secs {
                        return Err(HostPoolBuildError::SpecTimeframeMismatch {
                            symbol: symbol.clone(),
                            strategy_id: strategy_id.clone(),
                            expected: *timeframe_secs,
                            actual: b.timeframe_secs,
                        });
                    }
                    let recovery_failed =
                        |reason: String| HostPoolBuildError::DurableRecoveryFailed {
                            symbol: symbol.clone(),
                            strategy_id: strategy_id.clone(),
                            reason,
                        };
                    let host = CapitalFractionRuntimeHost::recover(
                        pool,
                        &b.registry,
                        &b.contract,
                        b.scope,
                    )
                    .await
                    .map_err(|e| recovery_failed(e.to_string()))?;
                    if !matches!(
                        host.semantic_fingerprint(),
                        Ok(ref fp) if *fp == b.semantic_fingerprint
                    ) {
                        return Err(recovery_failed(
                            "recovered host fingerprint differs from the resolved deployment identity"
                                .to_string(),
                        ));
                    }
                    RuntimeSelectedStrategyHost::DurableCapitalFraction(Box::new(host))
                }
            };
            hosts.insert(key, host);
        }
        Ok(Self { hosts })
    }

    fn fresh_key(
        hosts: &BTreeMap<HostPoolKey, RuntimeSelectedStrategyHost>,
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
    ) -> Result<HostPoolKey, HostPoolBuildError> {
        let key = (symbol.to_string(), strategy_id.to_string(), timeframe_secs);
        if hosts.contains_key(&key) {
            return Err(HostPoolBuildError::DuplicateKey {
                symbol: symbol.to_string(),
                strategy_id: strategy_id.to_string(),
                timeframe_secs,
            });
        }
        Ok(key)
    }

    /// Number of hosts in the pool.
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Every key currently in the pool, in deterministic (BTreeMap) order.
    pub fn keys(&self) -> impl Iterator<Item = &HostPoolKey> {
        self.hosts.keys()
    }

    /// Look up the host for one exact key, mutably (dispatch needs `&mut`
    /// for `on_bar`).
    pub fn get_mut(
        &mut self,
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
    ) -> Option<&mut RuntimeSelectedStrategyHost> {
        self.hosts
            .get_mut(&(symbol.to_string(), strategy_id.to_string(), timeframe_secs))
    }

    pub fn contains_key(&self, symbol: &str, strategy_id: &str, timeframe_secs: i64) -> bool {
        self.hosts
            .contains_key(&(symbol.to_string(), strategy_id.to_string(), timeframe_secs))
    }
}

fn build_stateless_host(
    symbol: &str,
    strategy_id: &str,
    timeframe_secs: i64,
) -> Result<StrategyHost, HostPoolBuildError> {
    let registry = build_daemon_plugin_registry_for_symbol(symbol);
    let strategy = registry
        .instantiate_verified(strategy_id)
        .map_err(|e| match e {
            RegistryError::UnknownStrategy { .. } => HostPoolBuildError::UnknownStrategy {
                symbol: symbol.to_string(),
                strategy_id: strategy_id.to_string(),
            },
            other => HostPoolBuildError::RegistryInconsistent {
                symbol: symbol.to_string(),
                strategy_id: strategy_id.to_string(),
                reason: other.to_string(),
            },
        })?;

    let spec = strategy.spec();
    if spec.name != strategy_id {
        return Err(HostPoolBuildError::SpecNameMismatch {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            expected: strategy_id.to_string(),
            actual: spec.name,
        });
    }
    if spec.timeframe_secs != timeframe_secs {
        return Err(HostPoolBuildError::SpecTimeframeMismatch {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            expected: timeframe_secs,
            actual: spec.timeframe_secs,
        });
    }

    let mut host = StrategyHost::new(ShadowMode::Off);
    host.register(strategy)
        .map_err(|e| HostPoolBuildError::HostRegistrationFailed {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            reason: format!("{e:?}"),
        })?;
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_builds_an_empty_pool() {
        let pool = DynamicSelectionHostPool::build(&[]).expect("empty selection must succeed");
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    // Real built-in spec timeframes (mqk_strategy::engines): swing_momentum
    // = 86400 (1D), mean_reversion = 3600 (1H), volatility_breakout = 3600
    // (1H), intraday_scalper = 300 (5m). instantiate_verified/SpecTimeframeMismatch
    // require an exact match, so every fixture below uses a strategy's real
    // timeframe unless the test is specifically about a mismatch.

    #[test]
    fn two_symbols_get_independent_isolated_hosts() {
        let selected = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("MSFT".to_string(), "mean_reversion".to_string(), 3600),
        ];
        let mut pool = DynamicSelectionHostPool::build(&selected).expect("build must succeed");
        assert_eq!(pool.len(), 2);
        assert!(pool.get_mut("AAPL", "swing_momentum", 86400).is_some());
        assert!(pool.get_mut("MSFT", "mean_reversion", 3600).is_some());
        // Cross lookups must not find a host under the wrong key.
        assert!(pool.get_mut("AAPL", "mean_reversion", 3600).is_none());
        assert!(pool.get_mut("MSFT", "swing_momentum", 86400).is_none());
    }

    #[test]
    fn same_strategy_independently_instantiated_for_two_symbols() {
        let selected = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("MSFT".to_string(), "swing_momentum".to_string(), 86400),
        ];
        let pool = DynamicSelectionHostPool::build(&selected).expect("build must succeed");
        assert_eq!(pool.len(), 2, "same strategy_id, two symbols -> two hosts");
    }

    #[test]
    fn duplicate_key_is_refused() {
        let selected = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
        ];
        match DynamicSelectionHostPool::build(&selected) {
            Err(e) => assert_eq!(
                e,
                HostPoolBuildError::DuplicateKey {
                    symbol: "AAPL".to_string(),
                    strategy_id: "swing_momentum".to_string(),
                    timeframe_secs: 86400,
                }
            ),
            Ok(_) => panic!("expected DuplicateKey error"),
        }
    }

    #[test]
    fn same_symbol_strategy_different_timeframe_is_not_treated_as_duplicate_key() {
        // Identity is the full (symbol, strategy_id, timeframe_secs) triple
        // (R2, Phase 0R) -- two entries sharing symbol+strategy_id but
        // differing timeframe_secs must never be short-circuited by the
        // duplicate-key check. swing_momentum's real spec timeframe is
        // 86400; the second entry (300) necessarily fails
        // SpecTimeframeMismatch during instantiation, but critically *not*
        // DuplicateKey -- proving each was evaluated as its own distinct
        // identity, not collapsed on (symbol, strategy_id) alone.
        let selected = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("AAPL".to_string(), "swing_momentum".to_string(), 300),
        ];
        match DynamicSelectionHostPool::build(&selected) {
            Err(HostPoolBuildError::DuplicateKey { .. }) => {
                panic!("distinct timeframe_secs must never be treated as a duplicate key")
            }
            Err(HostPoolBuildError::SpecTimeframeMismatch { .. }) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("expected SpecTimeframeMismatch, got Ok"),
        }
    }

    #[test]
    fn unknown_strategy_id_is_refused() {
        let selected = vec![(
            "AAPL".to_string(),
            "totally_unknown_strategy".to_string(),
            86400,
        )];
        match DynamicSelectionHostPool::build(&selected) {
            Err(e) => assert_eq!(
                e,
                HostPoolBuildError::UnknownStrategy {
                    symbol: "AAPL".to_string(),
                    strategy_id: "totally_unknown_strategy".to_string(),
                }
            ),
            Ok(_) => panic!("expected UnknownStrategy error"),
        }
    }

    #[test]
    fn wrong_timeframe_for_a_real_strategy_is_refused() {
        // swing_momentum's real spec timeframe is 86400, not 1 second.
        let selected = vec![("AAPL".to_string(), "swing_momentum".to_string(), 1)];
        let result = DynamicSelectionHostPool::build(&selected);
        assert!(matches!(
            result,
            Err(HostPoolBuildError::SpecTimeframeMismatch { .. })
        ));
    }

    #[test]
    fn build_never_partially_populates_pool_on_failure() {
        // First key valid, second key invalid -- the whole build must fail,
        // not return a pool containing only the first host.
        let selected = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("AAPL".to_string(), "does_not_exist".to_string(), 86400),
        ];
        let result = DynamicSelectionHostPool::build(&selected);
        assert!(
            result.is_err(),
            "second key's failure must fail the whole build"
        );
    }

    #[test]
    fn input_order_does_not_change_the_resulting_pool_keys() {
        let forward = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 86400),
            ("MSFT".to_string(), "mean_reversion".to_string(), 3600),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();

        let pool_a = DynamicSelectionHostPool::build(&forward).unwrap();
        let pool_b = DynamicSelectionHostPool::build(&reversed).unwrap();
        let keys_a: Vec<&HostPoolKey> = pool_a.keys().collect();
        let keys_b: Vec<&HostPoolKey> = pool_b.keys().collect();
        assert_eq!(
            keys_a, keys_b,
            "pool key order must be input-order-independent"
        );
    }

    #[test]
    fn host_pool_build_never_dispatches_or_touches_outbox() {
        // Structural proof, not a runtime assertion: DynamicSelectionHostPool
        // and its build() function contain no reference to on_bar,
        // outbox_enqueue, or any broker/decision-submission symbol -- this
        // test exists as a discoverability anchor (grep target) alongside
        // the module doc's "not yet wired" claim, not a behavioral check
        // (there is nothing to dispatch to from this module in the first
        // place).
        let selected = vec![("AAPL".to_string(), "swing_momentum".to_string(), 86400)];
        let pool = DynamicSelectionHostPool::build(&selected).expect("build must succeed");
        assert_eq!(pool.len(), 1);
    }
}
