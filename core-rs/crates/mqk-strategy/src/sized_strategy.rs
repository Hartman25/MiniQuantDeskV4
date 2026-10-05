//! Capital-fraction sized strategy wrapper.
//!
//! Native engines are long/flat signal generators (their positive target only
//! marks "long"). This wrapper turns that direction into an exact
//! quantity under `FixedInitialCapitalFractionV1`:
//!
//! * flat -> long: `Q` is resolved ONCE through
//!   [`resolve_capital_fraction_target`] from the close of the last completed
//!   bar in the context (causal; never a future or fill price);
//! * while the inner strategy stays long, the same `Q` is re-emitted (no
//!   resize);
//! * any non-long inner decision for the symbol releases `Q`, so the next
//!   entry re-resolves from the SAME initial capital/fraction and the new
//!   causal close;
//! * a refusal emits a flat target and is recorded; it never falls back to a
//!   default quantity.
//!
//! The held `Q` is in-memory state. Backtest/Research use [`CapitalFractionSizedStrategy::new`]
//! (one continuous instance). A runtime that can restart must build the wrapper
//! with [`CapitalFractionSizedStrategy::new_recoverable`] from a validated durable
//! snapshot, and persist [`SizingStateHandle::drain_transitions`] before it acts
//! on the bar's output; see `sizing_state`. `new_recoverable` also seeds stateful
//! engines with the Active records' entry anchors via
//! [`Strategy::restore_held_positions`], so an engine's hold state is never
//! rebuilt from a bounded window alone.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};

use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::sizing::{
    resolve_capital_fraction_target, CapitalFractionRefusal, CapitalFractionResolution,
    SizingPolicy, TargetSizing,
};
use crate::sizing_state::{
    HeldSizingContract, HeldSizingRecord, HeldSizingRecoveryError, HeldSizingScope,
    HeldSizingStatus, HeldSizingTransition, HELD_SIZING_STATE_VERSION,
};
use crate::{HeldPositionSeed, Strategy, StrategyContext, StrategySpec};

const WRAPPER_NAME: &str = "capital_fraction_sized_strategy";
const WRAPPER_VERSION: &str = "v1";

/// The one canonical semantic identity of a capital-fraction-wrapped strategy.
/// Backtest, Research registration and Paper/runtime resolution all derive the
/// wrapped fingerprint through this function; it takes no market-result value.
pub fn capital_fraction_semantic_fingerprint(
    inner_semantic_fingerprint: &str,
    allocation_fraction_bps: i64,
    initial_allocated_capital_micros: i64,
    caps: &TargetSizing,
) -> String {
    let mut b =
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, WRAPPER_NAME, WRAPPER_VERSION);
    b.push_str(inner_semantic_fingerprint)
        .push_str(crate::sizing::SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1)
        .push_i64(allocation_fraction_bps)
        .push_i64(initial_allocated_capital_micros)
        .push_str(&format!("asset_class:{:?}", caps.asset_class()))
        .push_opt_i64(caps.max_target_qty().map(|q| q.raw()))
        .push_opt_i64(caps.max_notional_usd());
    b.finish()
}

/// One resolved flat->long entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizedEntryRecord {
    pub symbol: String,
    /// `end_ts` of the completed bar whose close was the reference price.
    pub reference_bar_end_ts: i64,
    pub resolution: CapitalFractionResolution,
}

/// One refused flat->long entry (the strategy stayed flat).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizingRefusalRecord {
    pub symbol: String,
    pub reference_bar_end_ts: Option<i64>,
    pub refusal: CapitalFractionRefusal,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SizingAudit {
    pub entries: Vec<SizedEntryRecord>,
    pub refusals: Vec<SizingRefusalRecord>,
}

/// Shared handle to the audit trail written by the wrapper.
#[derive(Clone, Debug, Default)]
pub struct SizingAuditHandle(Arc<Mutex<SizingAudit>>);

impl SizingAuditHandle {
    pub fn snapshot(&self) -> SizingAudit {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut SizingAudit) -> R) -> Option<R> {
        self.0.lock().ok().map(|mut g| f(&mut g))
    }
}

/// Shared queue of held-state transitions awaiting durable persistence.
#[derive(Clone, Debug, Default)]
pub struct SizingStateHandle(Arc<Mutex<Vec<HeldSizingTransition>>>);

impl SizingStateHandle {
    /// Take every transition produced since the last drain, in order.
    pub fn drain_transitions(&self) -> Vec<HeldSizingTransition> {
        self.0
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }

    fn push(&self, t: HeldSizingTransition) {
        if let Ok(mut g) = self.0.lock() {
            g.push(t);
        }
    }
}

/// Durable-state bookkeeping present only for a recoverable wrapper.
struct Recoverable {
    scope: HeldSizingScope,
    /// Latest record per symbol (active or released): the generation floor.
    latest: BTreeMap<String, HeldSizingRecord>,
    state: SizingStateHandle,
}

pub struct CapitalFractionSizedStrategy {
    inner: Box<dyn Strategy>,
    allocation_fraction_bps: i64,
    initial_allocated_capital_micros: i64,
    caps: TargetSizing,
    held: BTreeMap<String, QtyMicros>,
    audit: SizingAuditHandle,
    recoverable: Option<Recoverable>,
}

impl CapitalFractionSizedStrategy {
    /// `policy` must be the capital-fraction policy; anything else, or an
    /// invalid fraction, is refused. Capital/price validity is enforced by the
    /// resolver at entry time (fail closed, recorded as a refusal).
    pub fn new(
        inner: Box<dyn Strategy>,
        policy: SizingPolicy,
        initial_allocated_capital_micros: i64,
        caps: TargetSizing,
    ) -> Result<(Self, SizingAuditHandle), CapitalFractionRefusal> {
        policy.validate()?;
        let allocation_fraction_bps = policy
            .allocation_fraction_bps()
            .ok_or(CapitalFractionRefusal::InvalidAllocationFractionBps { bps: 0 })?;
        let audit = SizingAuditHandle::default();
        Ok((
            Self {
                inner,
                allocation_fraction_bps,
                initial_allocated_capital_micros,
                caps,
                held: BTreeMap::new(),
                audit: audit.clone(),
                recoverable: None,
            },
            audit,
        ))
    }

    /// Build a wrapper for a runtime that can restart. `snapshot` is the
    /// durable state read for `scope` (an empty snapshot is an explicit "no
    /// held entries"); every record is proven against the contract and one
    /// record per symbol is required. An `Active` record restores the held
    /// `Q` exactly; a `Released` record only raises the entry generation.
    pub fn new_recoverable(
        inner: Box<dyn Strategy>,
        policy: SizingPolicy,
        initial_allocated_capital_micros: i64,
        caps: TargetSizing,
        scope: HeldSizingScope,
        snapshot: Vec<HeldSizingRecord>,
    ) -> Result<(Self, SizingAuditHandle, SizingStateHandle), HeldSizingRecoveryError> {
        let (mut wrapper, audit) = Self::new(inner, policy, initial_allocated_capital_micros, caps)
            .map_err(|_| HeldSizingRecoveryError::InvalidContract {
                reason: "policy is not a valid capital-fraction policy",
            })?;
        let contract = HeldSizingContract {
            scope: scope.clone(),
            allocation_fraction_bps: wrapper.allocation_fraction_bps,
            initial_allocated_capital_micros,
            caps,
        };
        contract.validate_scope()?;
        let mut latest: BTreeMap<String, HeldSizingRecord> = BTreeMap::new();
        for record in snapshot {
            contract.validate_record(&record)?;
            if latest.contains_key(&record.symbol) {
                return Err(HeldSizingRecoveryError::DuplicateSymbol {
                    symbol: record.symbol,
                });
            }
            if record.status == HeldSizingStatus::Active {
                wrapper.held.insert(
                    record.symbol.clone(),
                    QtyMicros::new(record.resolved_target_qty_micros),
                );
            }
            latest.insert(record.symbol.clone(), record);
        }
        let seeds: Vec<HeldPositionSeed> = latest
            .values()
            .filter(|r| r.status == HeldSizingStatus::Active)
            .map(|r| HeldPositionSeed {
                symbol: r.symbol.clone(),
                entry_bar_end_ts: r.reference_bar_end_ts,
            })
            .collect();
        wrapper.inner.restore_held_positions(&seeds);
        let state = SizingStateHandle::default();
        wrapper.recoverable = Some(Recoverable {
            scope,
            latest,
            state: state.clone(),
        });
        Ok((wrapper, audit, state))
    }

    fn note_entry(&mut self, symbol: &str, end_ts: i64, r: &CapitalFractionResolution) {
        let (bps, caps) = (self.allocation_fraction_bps, self.caps);
        let Some(rec) = self.recoverable.as_mut() else {
            return;
        };
        let generation = rec
            .latest
            .get(symbol)
            .map(|l| l.entry_generation)
            .unwrap_or(0)
            + 1;
        let record = HeldSizingRecord {
            deployment_id: rec.scope.deployment_id.clone(),
            strategy_id: rec.scope.strategy_id.clone(),
            symbol: symbol.to_string(),
            state_version: HELD_SIZING_STATE_VERSION,
            entry_generation: generation,
            status: HeldSizingStatus::Active,
            policy_id: crate::sizing::SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1.to_string(),
            allocation_fraction_bps: bps,
            initial_allocated_capital_micros: r.initial_allocated_capital_micros,
            max_target_qty_micros: caps.max_target_qty().map(|q| q.raw()),
            max_notional_usd: caps.max_notional_usd(),
            resolved_target_qty_micros: r.resolved_target_qty.raw(),
            reference_bar_end_ts: end_ts,
            reference_price_micros: r.causal_reference_price_micros,
        };
        rec.latest.insert(symbol.to_string(), record.clone());
        rec.state.push(HeldSizingTransition::Entered(record));
    }

    fn note_release(&mut self, symbol: &str) {
        let Some(rec) = self.recoverable.as_mut() else {
            return;
        };
        if let Some(latest) = rec.latest.get_mut(symbol) {
            if latest.status == HeldSizingStatus::Active {
                latest.status = HeldSizingStatus::Released;
                rec.state
                    .push(HeldSizingTransition::Released(latest.clone()));
            }
        }
    }
}

impl Strategy for CapitalFractionSizedStrategy {
    fn spec(&self) -> StrategySpec {
        self.inner.spec()
    }

    fn semantic_fingerprint(&self) -> String {
        capital_fraction_semantic_fingerprint(
            &self.inner.semantic_fingerprint(),
            self.allocation_fraction_bps,
            self.initial_allocated_capital_micros,
            &self.caps,
        )
    }

    fn empty_output_is_noop(&self) -> bool {
        self.inner.empty_output_is_noop()
    }

    fn required_history_bars(&self) -> usize {
        self.inner.required_history_bars()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let inner_out = self.inner.on_bar(ctx);
        if inner_out.targets.is_empty() && self.inner.empty_output_is_noop() {
            return inner_out;
        }
        let reference = ctx.recent.last().filter(|b| b.is_complete);
        let mut targets = Vec::with_capacity(inner_out.targets.len());
        let mut still_long: Vec<String> = Vec::new();

        for t in inner_out.targets {
            if !t.qty.is_positive() {
                targets.push(t);
                continue;
            }
            if let Some(q) = self.held.get(&t.symbol).copied() {
                still_long.push(t.symbol.clone());
                targets.push(TargetPosition::new(t.symbol, q));
                continue;
            }
            let resolved = match reference {
                Some(bar) => resolve_capital_fraction_target(
                    self.allocation_fraction_bps,
                    self.initial_allocated_capital_micros,
                    bar.close_micros,
                    &self.caps,
                )
                .map(|r| (bar.end_ts, r)),
                None => Err(CapitalFractionRefusal::NoCompletedReferenceBar),
            };
            match resolved {
                Ok((end_ts, resolution)) => {
                    let q = resolution.resolved_target_qty;
                    self.held.insert(t.symbol.clone(), q);
                    self.note_entry(&t.symbol, end_ts, &resolution);
                    still_long.push(t.symbol.clone());
                    self.audit.with(|a| {
                        a.entries.push(SizedEntryRecord {
                            symbol: t.symbol.clone(),
                            reference_bar_end_ts: end_ts,
                            resolution,
                        })
                    });
                    targets.push(TargetPosition::new(t.symbol, q));
                }
                Err(refusal) => {
                    self.audit.with(|a| {
                        a.refusals.push(SizingRefusalRecord {
                            symbol: t.symbol.clone(),
                            reference_bar_end_ts: reference.map(|b| b.end_ts),
                            refusal,
                        })
                    });
                    targets.push(TargetPosition::new(t.symbol, QtyMicros::ZERO));
                }
            }
        }

        // Complete-target semantics: a held symbol the inner strategy no longer
        // targets long (flat, absent, or short) is released.
        let released: Vec<String> = self
            .held
            .keys()
            .filter(|sym| !still_long.contains(sym))
            .cloned()
            .collect();
        for sym in &released {
            self.held.remove(sym);
            self.note_release(sym);
        }
        StrategyOutput { targets }
    }
}
