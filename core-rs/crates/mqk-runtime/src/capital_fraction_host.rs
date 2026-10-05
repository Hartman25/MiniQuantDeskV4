//! Restart-recoverable runtime host for `FixedInitialCapitalFractionV1`.
//!
//! The pure sizing math stays in `mqk_strategy` (the shared resolver and the
//! wrapper); this module owns the runtime state machine around it:
//!
//! 1. [`CapitalFractionRuntimeHost::recover`] reads the durable held-state
//!    snapshot for `(deployment, strategy)`, proves every record against the
//!    deployment contract, and only then builds the wrapper. Any foreign,
//!    stale, malformed or contract-mismatched record refuses the build.
//! 2. Evaluation is two-phase. [`CapitalFractionRuntimeHost::prepare_bar`]
//!    runs the strategy and stages its held-state transitions;
//!    [`commit_prepared_batch`] persists the staged transitions of one or more
//!    hosts in ONE transaction BEFORE any result is released, so a caller can
//!    never submit a decision whose sizing state is not durable, and a
//!    multi-binding tick never partially commits. If persistence fails or the
//!    prepared bar is aborted the host is poisoned: memory and DB may
//!    disagree, so it refuses further bars until rebuilt from the DB.
//!    [`CapitalFractionRuntimeHost::on_bar_durable`] is the single-host
//!    prepare-then-commit.
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
    pending: Option<Vec<HeldSizingTransitionRow>>,
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
            pending: None,
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

    /// True while a prepared bar's transitions are staged and not yet
    /// committed or aborted.
    pub fn has_pending_commit(&self) -> bool {
        self.pending.is_some()
    }

    /// Phase 1: run the strategy on `ctx` and stage the held-state
    /// transitions it produced WITHOUT writing the database. The returned
    /// result must not be released until [`commit_prepared_batch`] succeeds.
    /// The host refuses another bar until the staged transitions are
    /// finalized by that commit or the host is poisoned by an abort. An
    /// `on_bar` failure poisons the host: its in-memory held state may have
    /// advanced past the database.
    pub fn prepare_bar(
        &mut self,
        ctx: &StrategyContext,
    ) -> Result<StrategyBarResult, CapitalFractionRuntimeError> {
        if let Some(reason) = &self.poisoned {
            return Err(err(format!(
                "capital-fraction host is poisoned and must be rebuilt from durable state: {reason}"
            )));
        }
        if self.pending.is_some() {
            let reason = "a prepared bar was never committed or aborted".to_string();
            self.poisoned = Some(reason.clone());
            return Err(err(reason));
        }
        // Poisoned for the duration of the evaluation so an unwinding
        // `on_bar` (panic) leaves the host unusable; cleared only on success.
        self.poisoned = Some("bar evaluation did not complete".to_string());
        match self.host.on_bar(ctx) {
            Ok(result) => {
                self.pending = Some(transition_rows(self.state.drain_transitions()));
                self.poisoned = None;
                Ok(result)
            }
            Err(e) => {
                self.state.drain_transitions();
                let reason = format!("on_bar failed: {e:?}");
                self.poisoned = Some(reason.clone());
                Err(err(reason))
            }
        }
    }

    /// Invalidate a prepared bar that will never be committed (its memory
    /// state is ahead of the database). No-op when nothing is staged.
    pub fn abort_prepared(&mut self, reason: &str) {
        if self.pending.take().is_some() && self.poisoned.is_none() {
            self.poisoned = Some(format!("prepared bar aborted: {reason}"));
        }
    }

    /// Run the strategy on `ctx`, persist the resulting held-state
    /// transitions (idempotent, one transaction), then return the result.
    pub async fn on_bar_durable(
        &mut self,
        pool: &PgPool,
        ctx: &StrategyContext,
        now_utc: DateTime<Utc>,
    ) -> Result<StrategyBarResult, CapitalFractionRuntimeError> {
        let result = self.prepare_bar(ctx)?;
        commit_prepared_batch(pool, &mut [self], now_utc).await?;
        Ok(result)
    }
}

/// Phase 2: persist every staged transition of `hosts`, in slice order, in ONE
/// `held_sizing_apply` transaction, then finalize the hosts. On failure the
/// transaction has rolled back, so no host's transitions are durable; every
/// host is poisoned and the caller must release no result.
pub async fn commit_prepared_batch(
    pool: &PgPool,
    hosts: &mut [&mut CapitalFractionRuntimeHost],
    now_utc: DateTime<Utc>,
) -> Result<(), CapitalFractionRuntimeError> {
    let mut batch = Vec::new();
    for h in hosts.iter() {
        match &h.pending {
            Some(rows) if h.poisoned.is_none() => batch.extend(rows.iter().cloned()),
            _ => {
                return Err(err(
                    "a host in the commit batch has no prepared bar or is poisoned",
                ))
            }
        }
    }
    if !batch.is_empty() {
        if let Err(e) = held_sizing_apply(pool, &batch, now_utc).await {
            let reason = format!("held sizing persistence failed: {e:#}");
            for h in hosts.iter_mut() {
                h.pending = None;
                h.poisoned = Some(reason.clone());
            }
            return Err(err(reason));
        }
    }
    for h in hosts.iter_mut() {
        h.pending = None;
    }
    Ok(())
}
