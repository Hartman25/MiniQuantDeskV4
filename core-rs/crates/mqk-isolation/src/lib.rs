//! mqk-isolation
//!
//! PATCH 13 – Engine Isolation Layer
//!
//! Responsibilities:
//! - Engine-scoped broker key loading (MAIN vs EXP)
//! - Allocation cap enforcement helpers (per-engine)
//! - Minimal in-memory engine scoping primitives to prevent cross-engine bleed

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// Canonical micros scale used across the workspace.
pub const MICROS_SCALE: i64 = 1_000_000;

/// Engine identity (stable string: e.g. MAIN, EXP).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineId(pub String);

impl EngineId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-engine isolation policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineIsolation {
    /// Max gross exposure multiplier vs equity.
    /// Example: 1.0 => 1_000_000 micros.
    pub max_gross_exposure_mult_micros: i64,

    /// Broker key environment variable names (engine-scoped).
    pub broker_api_key_env: String,
    pub broker_api_secret_env: String,
}

impl EngineIsolation {
    /// Build from canonical config JSON (produced by mqk-config).
    ///
    /// Required fields:
    /// - engine.engine_id
    /// - broker.keys_env.api_key
    /// - broker.keys_env.api_secret
    ///
    /// Optional:
    /// - risk.max_gross_exposure (float or string); default=1.0
    pub fn from_config_json(cfg: &Value) -> Result<(EngineId, Self)> {
        let engine_id = cfg
            .pointer("/engine/engine_id")
            .and_then(Value::as_str)
            .context("config missing engine.engine_id")?;

        let api_key_env = cfg
            .pointer("/broker/keys_env/api_key")
            .and_then(Value::as_str)
            .context("config missing broker.keys_env.api_key")?;

        let api_secret_env = cfg
            .pointer("/broker/keys_env/api_secret")
            .and_then(Value::as_str)
            .context("config missing broker.keys_env.api_secret")?;

        // Enforce engine-scoped key names: MAIN vs EXP must not share a generic key.
        // Rule: the env var name must include the engine id token.
        // This keeps separation enforceable at config level.
        let token = engine_id.to_ascii_uppercase();
        if !api_key_env.to_ascii_uppercase().contains(&token) {
            return Err(anyhow!(
                "broker.keys_env.api_key must include engine_id token '{token}' (got '{api_key_env}')"
            ));
        }
        if !api_secret_env.to_ascii_uppercase().contains(&token) {
            return Err(anyhow!(
                "broker.keys_env.api_secret must include engine_id token '{token}' (got '{api_secret_env}')"
            ));
        }

        // risk.max_gross_exposure is a multiplier (e.g. 1.0). Accept number or string.
        let mult = cfg.pointer("/risk/max_gross_exposure");
        let mult_f64 = match mult {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(1.0),
            Some(Value::String(s)) => s.parse::<f64>().unwrap_or(1.0),
            _ => 1.0,
        };
        if !(0.0..=100.0).contains(&mult_f64) {
            return Err(anyhow!(
                "risk.max_gross_exposure out of bounds (0..=100): {mult_f64}"
            ));
        }

        let mult_micros = (mult_f64 * MICROS_SCALE as f64).round() as i64;

        Ok((
            EngineId::new(engine_id),
            Self {
                max_gross_exposure_mult_micros: mult_micros,
                broker_api_key_env: api_key_env.to_string(),
                broker_api_secret_env: api_secret_env.to_string(),
            },
        ))
    }

    /// Load broker credentials from environment using the engine-scoped env var names.
    pub fn load_broker_keys_from_env(&self) -> Result<(String, String)> {
        let key = std::env::var(&self.broker_api_key_env)
            .with_context(|| format!("missing env {}", self.broker_api_key_env))?;
        let secret = std::env::var(&self.broker_api_secret_env)
            .with_context(|| format!("missing env {}", self.broker_api_secret_env))?;
        Ok((key, secret))
    }
}

/// Compute max gross exposure allowed, in micros, from equity and multiplier.
pub fn max_gross_exposure_allowed_micros(equity_micros: i64, mult_micros: i64) -> i64 {
    // equity * mult / 1e6
    let num = equity_micros as i128 * mult_micros as i128;
    let den = MICROS_SCALE as i128;
    let v = num / den;
    if v > i64::MAX as i128 {
        i64::MAX
    } else if v < 0 {
        0
    } else {
        v as i64
    }
}

/// Allocation cap enforcement helper.
///
/// Returns Ok(()) if (current_gross + proposed_increment) <= allowed.
pub fn enforce_allocation_cap_micros(
    equity_micros: i64,
    current_gross_exposure_micros: i64,
    proposed_gross_increment_micros: i64,
    max_gross_exposure_mult_micros: i64,
) -> Result<(), AllocationCapBreach> {
    let allowed = max_gross_exposure_allowed_micros(equity_micros, max_gross_exposure_mult_micros);
    let next = current_gross_exposure_micros as i128 + proposed_gross_increment_micros as i128;
    if next > allowed as i128 {
        Err(AllocationCapBreach {
            equity_micros,
            current_gross_exposure_micros,
            proposed_gross_increment_micros,
            max_gross_exposure_allowed_micros: allowed,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationCapBreach {
    pub equity_micros: i64,
    pub current_gross_exposure_micros: i64,
    pub proposed_gross_increment_micros: i64,
    pub max_gross_exposure_allowed_micros: i64,
}

/// Minimal engine-keyed store to avoid cross-engine state bleed in-memory.
#[derive(Clone, Debug)]
pub struct EngineStore<T> {
    inner: BTreeMap<EngineId, T>,
}

impl<T> EngineStore<T> {
    pub fn new() -> Self {
        Self {
            inner: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, engine: EngineId, value: T) {
        self.inner.insert(engine, value);
    }

    pub fn get(&self, engine: &EngineId) -> Option<&T> {
        self.inner.get(engine)
    }

    pub fn get_mut(&mut self, engine: &EngineId) -> Option<&mut T> {
        self.inner.get_mut(engine)
    }
}
