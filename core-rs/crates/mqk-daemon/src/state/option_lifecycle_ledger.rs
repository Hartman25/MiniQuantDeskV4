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

use std::sync::Arc;

use sqlx::PgPool;

use mqk_db::{BrokerAccountAuthority, LifecycleJournalEntry};
use mqk_portfolio::{
    try_apply_entry, LedgerEntry, LifecycleAdjustment, PortfolioState, QtyMicros,
    UnderlyingAdjustment,
};

use super::option_lifecycle_ingestion::OPTION_LIFECYCLE_EXECUTION_DOMAIN;
use super::OptionLifecycleActivityFetcher;

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

/// The provider account the lifecycle fetcher is authenticated against, or
/// `None` when no fetcher is configured or the account endpoint cannot prove
/// one. `None` is "not established", never an account.
pub fn lifecycle_fetcher_account(
    fetcher: Option<&Arc<dyn OptionLifecycleActivityFetcher>>,
) -> Option<BrokerAccountAuthority> {
    fetcher.and_then(|f| f.broker_account_authority().ok())
}

/// The not-yet-subsumed journal entries of exactly `account`, in journal order.
///
/// `None` means no provider account identity is established: another account's
/// entries under the same deployment mode must never reach this ledger, so this
/// refuses whenever ANY account of `deployment_mode` holds an unsubsumed entry,
/// and otherwise returns nothing (nothing exists to omit).
pub async fn load_unsubsumed_journal(
    pool: &PgPool,
    account: Option<&BrokerAccountAuthority>,
    deployment_mode: &str,
) -> anyhow::Result<Vec<LifecycleJournalEntry>> {
    let Some(account) = account else {
        let unscoped = mqk_db::count_unsubsumed_lifecycle_journal_for_deployment_mode(
            pool,
            OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
            deployment_mode,
        )
        .await?;
        if unscoped > 0 {
            anyhow::bail!(
                "{unscoped} unsubsumed lifecycle adjustment(s) exist under deployment mode \
                 {deployment_mode:?} but the provider account is not established"
            );
        }
        return Ok(Vec::new());
    };
    mqk_db::list_unsubsumed_lifecycle_journal(
        pool,
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
        account,
    )
    .await
}

/// Replay the journal of exactly `account` into a freshly recovered portfolio
/// (see [`load_unsubsumed_journal`] for the `None` semantics).
pub async fn replay_lifecycle_journal_into_portfolio(
    pool: &PgPool,
    account: Option<&BrokerAccountAuthority>,
    deployment_mode: &str,
    portfolio: &mut PortfolioState,
) -> anyhow::Result<LifecycleReplay> {
    let entries = load_unsubsumed_journal(pool, account, deployment_mode)
        .await
        .map_err(|e| {
            anyhow::anyhow!("replay_lifecycle_journal_into_portfolio: refusing recovery: {e}")
        })?;
    let mut replay = LifecycleReplay::default();
    for entry in &entries {
        let adjustment = journal_entry_to_ledger_adjustment(entry).map_err(|e| {
            anyhow::anyhow!("replay_lifecycle_journal_into_portfolio: refusing recovery: {e}")
        })?;
        try_apply_entry(portfolio, LedgerEntry::LifecycleAdjustment(adjustment)).map_err(|e| {
            anyhow::anyhow!(
                "replay_lifecycle_journal_into_portfolio: refusing recovery: journal entry {} \
                 cannot be applied exactly: {e}",
                entry.journal_seq
            )
        })?;
        replay.applied_ids.push(entry.economic_apply_id.clone());
    }
    Ok(replay)
}
