//! Restart-recoverable runtime host for `FixedInitialCapitalFractionV1`.
//!
//! The pure sizing math stays in `mqk_strategy` (the shared resolver and the
//! wrapper); this module owns the runtime state machine around it:
//!
//! 1. [`CapitalFractionRuntimeHost::recover`] reads the durable held-state
//!    snapshot for `(deployment, strategy)`, proves every record against the
//!    deployment contract, and only then builds the wrapper. Any foreign,
//!    stale, malformed or contract-mismatched record refuses the build.
//! 2. [`CapitalFractionRuntimeHost::on_bar_durable`] runs the strategy, then
//!    persists the held-state transitions it produced BEFORE returning the
//!    result, so a caller can never submit a decision whose sizing state is not
//!    durable. If persistence fails the host is poisoned: memory and DB may
//!    disagree, so it refuses further bars until rebuilt from the DB.
//!
//! Nothing here touches a broker, reads broker equity, or activates Paper.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::held_sizing_state::{
    held_sizing_apply, held_sizing_fetch, HeldSizingRow, HeldSizingTransitionRow,
    HELD_SIZING_STATUS_ACTIVE, HELD_SIZING_STATUS_RELEASED,
};
use mqk_strategy::{
    CapitalFractionSizedStrategy, HeldSizingRecord, HeldSizingScope, HeldSizingStatus,
    HeldSizingTransition, PluginRegistry, RestartRecovery, ShadowMode, SizingAudit,
    SizingAuditHandle, SizingStateHandle, StrategyBarResult, StrategyContext, StrategyHost,
};

use crate::native_strategy::CapitalFractionDeploymentContract;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalFractionRuntimeError(pub String);

impl std::fmt::Display for CapitalFractionRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CapitalFractionRuntimeError {}

fn err(m: impl Into<String>) -> CapitalFractionRuntimeError {
    CapitalFractionRuntimeError(m.into())
}

fn record_from_row(row: HeldSizingRow) -> Result<HeldSizingRecord, CapitalFractionRuntimeError> {
    let status = HeldSizingStatus::parse(&row.status).ok_or_else(|| {
        err(format!(
            "held sizing row has unknown status '{}'",
            row.status
        ))
    })?;
    Ok(HeldSizingRecord {
        deployment_id: row.deployment_id,
        strategy_id: row.strategy_id,
        symbol: row.symbol,
        state_version: row.state_version,
        entry_generation: row.entry_generation,
        status,
        policy_id: row.policy_id,
        allocation_fraction_bps: row.allocation_fraction_bps,
        initial_allocated_capital_micros: row.initial_allocated_capital_micros,
        max_target_qty_micros: row.max_target_qty_micros,
        max_notional_usd: row.max_notional_usd,
        resolved_target_qty_micros: row.resolved_target_qty_micros,
        reference_bar_end_ts: row.reference_bar_end_ts,
        reference_price_micros: row.reference_price_micros,
    })
}

fn row_from_record(r: &HeldSizingRecord) -> HeldSizingRow {
    HeldSizingRow {
        deployment_id: r.deployment_id.clone(),
        strategy_id: r.strategy_id.clone(),
        symbol: r.symbol.clone(),
        state_version: r.state_version,
        entry_generation: r.entry_generation,
        status: match r.status {
            HeldSizingStatus::Active => HELD_SIZING_STATUS_ACTIVE,
            HeldSizingStatus::Released => HELD_SIZING_STATUS_RELEASED,
        }
        .to_string(),
        policy_id: r.policy_id.clone(),
        allocation_fraction_bps: r.allocation_fraction_bps,
        initial_allocated_capital_micros: r.initial_allocated_capital_micros,
        max_target_qty_micros: r.max_target_qty_micros,
        max_notional_usd: r.max_notional_usd,
        resolved_target_qty_micros: r.resolved_target_qty_micros,
        reference_bar_end_ts: r.reference_bar_end_ts,
        reference_price_micros: r.reference_price_micros,
    }
}

fn transition_rows(ts: Vec<HeldSizingTransition>) -> Vec<HeldSizingTransitionRow> {
    ts.into_iter()
        .map(|t| match t {
            HeldSizingTransition::Entered(r) => {
                HeldSizingTransitionRow::Entered(row_from_record(&r))
            }
            HeldSizingTransition::Released(r) => {
                HeldSizingTransitionRow::Released(row_from_record(&r))
            }
        })
        .collect()
}

pub struct CapitalFractionRuntimeHost {
    host: StrategyHost,
    state: SizingStateHandle,
    audit: SizingAuditHandle,
    poisoned: Option<String>,
}

impl CapitalFractionRuntimeHost {
    /// Recover the durable snapshot for `scope`, prove it against `contract`,
    /// and build the host. `registry` must be the capital-fraction registry
    /// (`DurableStateRequired` entries); a stateless entry is refused.
    pub async fn recover(
        pool: &PgPool,
        registry: &PluginRegistry,
        contract: &CapitalFractionDeploymentContract,
        scope: HeldSizingScope,
    ) -> Result<Self, CapitalFractionRuntimeError> {
        let (inner, recovery) = registry
            .instantiate_for_identity(&scope.strategy_id)
            .map_err(|e| err(format!("strategy cannot be instantiated: {e}")))?;
        if recovery != RestartRecovery::DurableStateRequired {
            return Err(err(
                "strategy is not registered under the capital-fraction durable-state contract",
            ));
        }
        let rows = held_sizing_fetch(pool, &scope.deployment_id, &scope.strategy_id)
            .await
            .map_err(|e| err(format!("held sizing state unavailable: {e:#}")))?;
        let snapshot = rows
            .into_iter()
            .map(record_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let (wrapper, audit, state) = CapitalFractionSizedStrategy::new_recoverable(
            inner,
            contract.policy,
            contract.allocated_capital_micros,
            contract.caps,
            scope,
            snapshot,
        )
        .map_err(|e| err(format!("held sizing recovery refused: {e}")))?;
        let mut host = StrategyHost::new(ShadowMode::Off);
        host.register(Box::new(wrapper))
            .map_err(|e| err(format!("host registration failed: {e:?}")))?;
        Ok(Self {
            host,
            state,
            audit,
            poisoned: None,
        })
    }

    /// Wrapper semantic fingerprint (equals the canonical Backtest one).
    pub fn semantic_fingerprint(&self) -> Result<String, CapitalFractionRuntimeError> {
        self.host
            .semantic_fingerprint()
            .map_err(|e| err(format!("fingerprint unavailable: {e:?}")))
    }

    pub fn audit_snapshot(&self) -> SizingAudit {
        self.audit.snapshot()
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// Run the strategy on `ctx`, persist the resulting held-state
    /// transitions (idempotent, one transaction), then return the result.
    pub async fn on_bar_durable(
        &mut self,
        pool: &PgPool,
        ctx: &StrategyContext,
        now_utc: DateTime<Utc>,
    ) -> Result<StrategyBarResult, CapitalFractionRuntimeError> {
        if let Some(reason) = &self.poisoned {
            return Err(err(format!(
                "capital-fraction host is poisoned and must be rebuilt from durable state: {reason}"
            )));
        }
        let result = self
            .host
            .on_bar(ctx)
            .map_err(|e| err(format!("on_bar failed: {e:?}")))?;
        let transitions = transition_rows(self.state.drain_transitions());
        if !transitions.is_empty() {
            if let Err(e) = held_sizing_apply(pool, &transitions, now_utc).await {
                let reason = format!("held sizing persistence failed: {e:#}");
                self.poisoned = Some(reason.clone());
                return Err(err(reason));
            }
        }
        Ok(result)
    }
}
