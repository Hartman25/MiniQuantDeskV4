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

use mqk_runtime::native_strategy::build_daemon_plugin_registry_for_symbol;
use mqk_strategy::{RegistryError, ShadowMode, StrategyHost};

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
        }
    }
}

/// One isolated, single-registration [`StrategyHost`] per selected
/// `(symbol, strategy_id, timeframe_secs)` key, for exactly one active run.
pub struct DynamicSelectionHostPool {
    hosts: BTreeMap<HostPoolKey, StrategyHost>,
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
        let mut hosts: BTreeMap<HostPoolKey, StrategyHost> = BTreeMap::new();

        for (symbol, strategy_id, timeframe_secs) in selected {
            let key = (symbol.clone(), strategy_id.clone(), *timeframe_secs);
            if hosts.contains_key(&key) {
                return Err(HostPoolBuildError::DuplicateKey {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    timeframe_secs: *timeframe_secs,
                });
            }

            let registry = build_daemon_plugin_registry_for_symbol(symbol);
            let strategy = registry
                .instantiate_verified(strategy_id)
                .map_err(|e| match e {
                    RegistryError::UnknownStrategy { .. } => HostPoolBuildError::UnknownStrategy {
                        symbol: symbol.clone(),
                        strategy_id: strategy_id.clone(),
                    },
                    other => HostPoolBuildError::RegistryInconsistent {
                        symbol: symbol.clone(),
                        strategy_id: strategy_id.clone(),
                        reason: other.to_string(),
                    },
                })?;

            let spec = strategy.spec();
            if spec.name != *strategy_id {
                return Err(HostPoolBuildError::SpecNameMismatch {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    expected: strategy_id.clone(),
                    actual: spec.name,
                });
            }
            if spec.timeframe_secs != *timeframe_secs {
                return Err(HostPoolBuildError::SpecTimeframeMismatch {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    expected: *timeframe_secs,
                    actual: spec.timeframe_secs,
                });
            }

            let mut host = StrategyHost::new(ShadowMode::Off);
            host.register(strategy)
                .map_err(|e| HostPoolBuildError::HostRegistrationFailed {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    reason: format!("{e:?}"),
                })?;

            hosts.insert(key, host);
        }

        Ok(Self { hosts })
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
    ) -> Option<&mut StrategyHost> {
        self.hosts
            .get_mut(&(symbol.to_string(), strategy_id.to_string(), timeframe_secs))
    }

    pub fn contains_key(&self, symbol: &str, strategy_id: &str, timeframe_secs: i64) -> bool {
        self.hosts
            .contains_key(&(symbol.to_string(), strategy_id.to_string(), timeframe_secs))
    }
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
