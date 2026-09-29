//! D2: consumption of the options-lifecycle adjustment journal by the
//! canonical mutable ledger.
//!
//! The journal (`sys_option_lifecycle_adjustment_journal`) is durable evidence;
//! `mqk_portfolio::LedgerEntry::LifecycleAdjustment` is how that evidence
//! reaches `PortfolioState`. This module is the one translation and the one
//! recovery replay:
//!
//! - [`journal_entry_to_ledger_adjustment`] -- exact, total translation of a
//!   journal row to a ledger entry (no defaults, no recomputation);
//! - [`replay_lifecycle_journal_into_portfolio`] -- run recovery: apply every
//!   not-yet-baseline-subsumed entry of this deployment's accounts, in journal
//!   order, on top of the baseline + fills the recovery already applied,
//!   returning the replayed ids (the running orchestrator dedups on them).
//!
//! Exercise, assignment and expiration are never fills: nothing here touches
//! the inbox, an order, or a `Fill`.

use sqlx::PgPool;

use mqk_db::LifecycleJournalEntry;
use mqk_portfolio::{
    apply_entry, LedgerEntry, LifecycleAdjustment, PortfolioState, QtyMicros, UnderlyingAdjustment,
};

use super::option_lifecycle_ingestion::OPTION_LIFECYCLE_EXECUTION_DOMAIN;

/// Translate one journal row into the canonical ledger adjustment. Refuses (no
/// partial value) a row whose shape violates the journal's own invariants.
pub fn journal_entry_to_ledger_adjustment(
    e: &LifecycleJournalEntry,
) -> Result<LifecycleAdjustment, String> {
    let underlying = match (
        e.underlying_qty_delta_micros,
        e.strike_micros,
        e.cash_delta_micros,
    ) {
        (Some(qty), Some(strike), Some(_)) => Some(UnderlyingAdjustment {
            symbol: e.underlying_symbol.clone(),
            qty_delta: QtyMicros::new(qty),
            basis_price_micros: strike,
        }),
        (None, None, None) => None,
        _ => {
            return Err(format!(
                "journal row {} has a partial settlement shape",
                e.economic_apply_id
            ))
        }
    };
    Ok(LifecycleAdjustment {
        economic_apply_id: e.economic_apply_id.clone(),
        option_symbol: e.option_symbol.clone(),
        option_qty_delta: QtyMicros::new(e.option_qty_delta_micros),
        underlying,
        cash_delta_micros: e.cash_delta_micros.unwrap_or(0),
    })
}

/// Result of a recovery replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleReplay {
    /// `economic_apply_id`s applied to the portfolio, in journal order.
    pub applied_ids: Vec<String>,
}

/// Replay the journal into a freshly recovered portfolio.
pub async fn replay_lifecycle_journal_into_portfolio(
    pool: &PgPool,
    deployment_mode: &str,
    portfolio: &mut PortfolioState,
) -> anyhow::Result<LifecycleReplay> {
    let entries = mqk_db::list_unsubsumed_lifecycle_journal(
        pool,
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
        deployment_mode,
    )
    .await?;
    let mut replay = LifecycleReplay::default();
    for entry in &entries {
        let adjustment = journal_entry_to_ledger_adjustment(entry).map_err(|e| {
            anyhow::anyhow!("replay_lifecycle_journal_into_portfolio: refusing recovery: {e}")
        })?;
        apply_entry(portfolio, LedgerEntry::LifecycleAdjustment(adjustment));
        replay.applied_ids.push(entry.economic_apply_id.clone());
    }
    Ok(replay)
}
