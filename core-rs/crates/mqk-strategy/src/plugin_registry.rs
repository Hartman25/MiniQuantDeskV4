//! Plugin Registry — catalogue of available strategies + metadata.
//!
//! # Purpose
//! [`StrategyHost`](crate::StrategyHost) manages a single *active* strategy.
//! `PluginRegistry` is the step before that: a catalogue of *available*
//! strategies, each represented by:
//!
//! - [`StrategyMeta`] — static metadata (name, version, timeframe, description).
//! - A [`StrategyFactory`] — a `Send + Sync` closure that produces a fresh
//!   `Box<dyn Strategy>` on demand.
//!
//! This separation means the runtime can enumerate registered strategies,
//! select one by name, instantiate it, and hand it to `StrategyHost::register`
//! without coupling discovery to execution.  It also lays the foundation for a
//! future dynamic plugin model (e.g. loading `.so`/`.dll` plugins at runtime).
//!
//! # Usage
//! ```ignore
//! let mut reg = PluginRegistry::new();
//! reg.register(
//!     StrategyMeta::new("my_strategy", "1.0.0", 60, "Simple momentum"),
//!     || Box::new(MyStrategy::default()),
//! ).unwrap();
//!
//! let strategy = reg.instantiate("my_strategy").unwrap();
//! host.register(strategy).unwrap();
//! ```
//!
//! # Determinism
//! The registry itself is deterministic — insertion order is preserved in
//! `list()` output.  Factory closures must be deterministic if reproducible
//! backtest replay is required (seed injection is the caller's responsibility).

use crate::{Strategy, StrategySpec};

// ---------------------------------------------------------------------------
// Factory type alias
// ---------------------------------------------------------------------------

/// A thread-safe factory closure that produces a fresh strategy instance.
///
/// `Send + Sync` is required so the registry can be shared across threads
/// (e.g. held in an `Arc` by the daemon).  The closure must not capture
/// non-`Send` state.
pub type StrategyFactory = Box<dyn Fn() -> Box<dyn Strategy> + Send + Sync>;

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Static metadata for a registered strategy.
///
/// Metadata is stored separately from the strategy instance so it can be
/// queried without instantiating the strategy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyMeta {
    /// Unique name used as the registry key.  Must be non-empty and contain
    /// only ASCII alphanumeric characters, hyphens, or underscores.
    pub name: String,

    /// Semver-style version string (e.g. `"1.0.0"`).  Not validated beyond
    /// non-empty; caller is responsible for format.
    pub version: String,

    /// The timeframe this strategy operates on, in seconds.
    /// Must match the `StrategySpec` returned by the instantiated strategy.
    pub timeframe_secs: i64,

    /// Human-readable description of the strategy.
    pub description: String,
}

impl StrategyMeta {
    /// Construct metadata, validating the name and timeframe.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        timeframe_secs: i64,
        description: impl Into<String>,
    ) -> Self {
        let name = name.into();
        debug_assert!(
            !name.trim().is_empty(),
            "StrategyMeta name must not be empty"
        );
        debug_assert!(
            timeframe_secs > 0,
            "StrategyMeta timeframe_secs must be > 0"
        );
        Self {
            name,
            version: version.into(),
            timeframe_secs,
            description: description.into(),
        }
    }

    /// Derive metadata directly from an instantiated strategy's spec.
    ///
    /// Useful when the strategy struct already carries its own identity.
    pub fn from_spec(
        spec: &StrategySpec,
        version: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: spec.name.clone(),
            version: version.into(),
            timeframe_secs: spec.timeframe_secs,
            description: description.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by [`PluginRegistry`] operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// A strategy with the given name is already registered.
    DuplicateName { name: String },
    /// No strategy with the given name is registered.
    UnknownStrategy { name: String },
    /// The strategy name is empty or contains only whitespace.
    EmptyName,
    /// The timeframe in the metadata does not match the instantiated strategy's spec.
    TimeframeMismatch {
        name: String,
        meta_secs: i64,
        spec_secs: i64,
    },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(f, "strategy '{name}' is already registered")
            }
            Self::UnknownStrategy { name } => {
                write!(f, "no strategy named '{name}' is registered")
            }
            Self::EmptyName => write!(f, "strategy name must not be empty"),
            Self::TimeframeMismatch {
                name,
                meta_secs,
                spec_secs,
            } => write!(
                f,
                "strategy '{name}': metadata timeframe {meta_secs}s != spec timeframe {spec_secs}s"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

// ---------------------------------------------------------------------------
// Registry entry (internal)
// ---------------------------------------------------------------------------

struct RegistryEntry {
    meta: StrategyMeta,
    factory: StrategyFactory,
}

// ---------------------------------------------------------------------------
// PluginRegistry
// ---------------------------------------------------------------------------

/// Catalogue of available strategies and their factories.
///
/// Maintains insertion order for deterministic `list()` output.
/// Names are compared case-sensitively.
pub struct PluginRegistry {
    /// Entries in insertion order.
    entries: Vec<RegistryEntry>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a strategy by metadata and factory closure.
    ///
    /// # Errors
    /// - [`RegistryError::EmptyName`] if `meta.name` is empty/whitespace.
    /// - [`RegistryError::DuplicateName`] if a strategy with the same name is
    ///   already registered.
    pub fn register<F>(&mut self, meta: StrategyMeta, factory: F) -> Result<(), RegistryError>
    where
        F: Fn() -> Box<dyn Strategy> + Send + Sync + 'static,
    {
        if meta.name.trim().is_empty() {
            return Err(RegistryError::EmptyName);
        }
        if self.contains(&meta.name) {
            return Err(RegistryError::DuplicateName {
                name: meta.name.clone(),
            });
        }
        self.entries.push(RegistryEntry {
            meta,
            factory: Box::new(factory),
        });
        Ok(())
    }

    /// Return `true` if a strategy with the given name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.meta.name == name)
    }

    /// Return the number of registered strategies.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no strategies are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return metadata for all registered strategies in insertion order.
    pub fn list(&self) -> Vec<&StrategyMeta> {
        self.entries.iter().map(|e| &e.meta).collect()
    }

    /// Look up metadata for a strategy by name.
    ///
    /// # Errors
    /// [`RegistryError::UnknownStrategy`] if the name is not found.
    pub fn lookup(&self, name: &str) -> Result<&StrategyMeta, RegistryError> {
        self.entries
            .iter()
            .find(|e| e.meta.name == name)
            .map(|e| &e.meta)
            .ok_or_else(|| RegistryError::UnknownStrategy {
                name: name.to_string(),
            })
    }

    /// Instantiate a strategy by name using its registered factory.
    ///
    /// Each call produces a **fresh** strategy instance — the factory is
    /// called anew every time.  This is intentional: strategies may carry
    /// mutable state (bar history, signals) that must not leak across runs.
    ///
    /// # Errors
    /// [`RegistryError::UnknownStrategy`] if the name is not found.
    pub fn instantiate(&self, name: &str) -> Result<Box<dyn Strategy>, RegistryError> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.meta.name == name)
            .ok_or_else(|| RegistryError::UnknownStrategy {
                name: name.to_string(),
            })?;
        Ok((entry.factory)())
    }

    /// Instantiate a strategy and verify that its `spec().timeframe_secs`
    /// matches the registered metadata.
    ///
    /// Use this when strict consistency between metadata and implementation is
    /// required (e.g. at promotion / live-arm time).
    ///
    /// # Errors
    /// - [`RegistryError::UnknownStrategy`] if the name is not found.
    /// - [`RegistryError::TimeframeMismatch`] if the spec doesn't match.
    pub fn instantiate_verified(&self, name: &str) -> Result<Box<dyn Strategy>, RegistryError> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.meta.name == name)
            .ok_or_else(|| RegistryError::UnknownStrategy {
                name: name.to_string(),
            })?;

        let strategy = (entry.factory)();
        let spec = strategy.spec();

        if spec.timeframe_secs != entry.meta.timeframe_secs {
            return Err(RegistryError::TimeframeMismatch {
                name: name.to_string(),
                meta_secs: entry.meta.timeframe_secs,
                spec_secs: spec.timeframe_secs,
            });
        }

        Ok(strategy)
    }

    /// Remove a registered strategy by name.
    ///
    /// Returns `true` if the strategy was found and removed, `false` if it
    /// was not registered.  Preserves insertion order of remaining entries.
    pub fn deregister(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.meta.name != name);
        self.entries.len() < before
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StrategyContext, StrategySpec};
    use mqk_execution::{StrategyOutput, TargetPosition};

    // Minimal concrete strategy for testing.
    struct FixedTargetStrategy {
        name: &'static str,
        timeframe_secs: i64,
        target_qty: i64,
    }

    impl Strategy for FixedTargetStrategy {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new(self.name, self.timeframe_secs)
        }

        fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
            StrategyOutput {
                targets: vec![TargetPosition {
                    symbol: "SPY".to_string(),
                    target_qty: self.target_qty,
                }],
            }
        }
    }

    fn make_meta(name: &str, tf: i64) -> StrategyMeta {
        StrategyMeta::new(name, "1.0.0", tf, "test strategy")
    }

    fn make_factory(
        name: &'static str,
        tf: i64,
        qty: i64,
    ) -> impl Fn() -> Box<dyn Strategy> + Send + Sync {
        move || {
            Box::new(FixedTargetStrategy {
                name,
                timeframe_secs: tf,
                target_qty: qty,
            })
        }
    }

    // --- Registration ---

    #[test]
    fn register_single_strategy_succeeds() {
        let mut reg = PluginRegistry::new();
        let result = reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 10));
        assert!(result.is_ok());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_duplicate_name_errors() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 10))
            .unwrap();
        let err = reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 20));
        assert_eq!(
            err,
            Err(RegistryError::DuplicateName {
                name: "alpha".to_string()
            })
        );
    }

    #[test]
    fn register_multiple_distinct_strategies() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 10))
            .unwrap();
        reg.register(make_meta("beta", 300), make_factory("beta", 300, 5))
            .unwrap();
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn register_empty_name_errors() {
        let mut reg = PluginRegistry::new();
        let meta = StrategyMeta {
            name: "".to_string(),
            version: "1.0.0".to_string(),
            timeframe_secs: 60,
            description: "bad".to_string(),
        };
        let err = reg.register(meta, make_factory("x", 60, 1));
        assert_eq!(err, Err(RegistryError::EmptyName));
    }

    // --- contains / len / is_empty ---

    #[test]
    fn contains_returns_true_for_registered() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();
        assert!(reg.contains("alpha"));
        assert!(!reg.contains("beta"));
    }

    #[test]
    fn new_registry_is_empty() {
        let reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    // --- list ---

    #[test]
    fn list_returns_entries_in_insertion_order() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();
        reg.register(make_meta("beta", 300), make_factory("beta", 300, 2))
            .unwrap();
        reg.register(make_meta("gamma", 3600), make_factory("gamma", 3600, 3))
            .unwrap();

        let names: Vec<&str> = reg.list().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
    }

    // --- lookup ---

    #[test]
    fn lookup_known_name_returns_meta() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();

        let meta = reg.lookup("alpha").unwrap();
        assert_eq!(meta.name, "alpha");
        assert_eq!(meta.timeframe_secs, 60);
    }

    #[test]
    fn lookup_unknown_name_errors() {
        let reg = PluginRegistry::new();
        let err = reg.lookup("ghost");
        assert_eq!(
            err,
            Err(RegistryError::UnknownStrategy {
                name: "ghost".to_string()
            })
        );
    }

    // --- instantiate ---

    #[test]
    fn instantiate_produces_fresh_strategy() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 42))
            .unwrap();

        let s = reg.instantiate("alpha").unwrap();
        assert_eq!(s.spec().name, "alpha");
        assert_eq!(s.spec().timeframe_secs, 60);
    }

    #[test]
    fn instantiate_unknown_errors() {
        let reg = PluginRegistry::new();
        let err = reg.instantiate("ghost");
        assert!(matches!(err, Err(RegistryError::UnknownStrategy { name }) if name == "ghost"));
    }

    #[test]
    fn instantiate_called_twice_produces_independent_instances() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();

        let s1 = reg.instantiate("alpha").unwrap();
        let s2 = reg.instantiate("alpha").unwrap();
        // Both have correct spec — independent instances.
        assert_eq!(s1.spec().name, s2.spec().name);
    }

    // --- instantiate_verified ---

    #[test]
    fn instantiate_verified_passes_when_timeframe_matches() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();

        let s = reg.instantiate_verified("alpha").unwrap();
        assert_eq!(s.spec().timeframe_secs, 60);
    }

    #[test]
    fn instantiate_verified_errors_on_mismatch() {
        let mut reg = PluginRegistry::new();
        // metadata says 60s but factory produces a 300s strategy
        reg.register(make_meta("alpha", 60), make_factory("alpha", 300, 1))
            .unwrap();

        let err = reg.instantiate_verified("alpha");
        assert!(matches!(
            err,
            Err(RegistryError::TimeframeMismatch {
                name,
                meta_secs: 60,
                spec_secs: 300,
            }) if name == "alpha"
        ));
    }

    // --- deregister ---

    #[test]
    fn deregister_removes_entry() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();
        reg.register(make_meta("beta", 300), make_factory("beta", 300, 2))
            .unwrap();

        assert!(reg.deregister("alpha"));
        assert!(!reg.contains("alpha"));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn deregister_unknown_returns_false() {
        let mut reg = PluginRegistry::new();
        assert!(!reg.deregister("ghost"));
    }

    #[test]
    fn deregister_preserves_insertion_order_of_remaining() {
        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 1))
            .unwrap();
        reg.register(make_meta("beta", 300), make_factory("beta", 300, 2))
            .unwrap();
        reg.register(make_meta("gamma", 3600), make_factory("gamma", 3600, 3))
            .unwrap();

        reg.deregister("beta");

        let names: Vec<&str> = reg.list().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["alpha", "gamma"]);
    }

    // --- StrategyMeta helpers ---

    #[test]
    fn meta_from_spec() {
        let spec = StrategySpec::new("my_strat", 300);
        let meta = StrategyMeta::from_spec(&spec, "2.1.0", "desc");
        assert_eq!(meta.name, "my_strat");
        assert_eq!(meta.timeframe_secs, 300);
        assert_eq!(meta.version, "2.1.0");
    }

    // --- Default ---

    #[test]
    fn default_produces_empty_registry() {
        let reg = PluginRegistry::default();
        assert!(reg.is_empty());
    }

    // --- Integration: registry → host ---

    #[test]
    fn registry_to_host_round_trip() {
        use crate::{ShadowMode, StrategyHost};

        let mut reg = PluginRegistry::new();
        reg.register(make_meta("alpha", 60), make_factory("alpha", 60, 10))
            .unwrap();

        let strategy = reg.instantiate("alpha").unwrap();

        let mut host = StrategyHost::new(ShadowMode::Off);
        host.register(strategy).unwrap();

        assert_eq!(host.spec().unwrap().name, "alpha");
    }
}
