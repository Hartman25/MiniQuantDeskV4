//! D3: broker agreement -> `RECONCILED`.
//!
//! After D2 applies an options-lifecycle adjustment the event is
//! `APPLIED_AWAITING_BROKER` and the gate stays closed. It advances to
//! `RECONCILED` (and only then clears the gate) when a FRESH, authenticated
//! broker snapshot agrees with the local ledger for BOTH the option contract
//! and its underlying, and the local ledger has actually absorbed the entry:
//!
//! - the running orchestrator's ledger has absorbed the entry (its
//!   `economic_apply_id` is in the orchestrator's applied set);
//! - the broker snapshot was captured strictly AFTER the apply (a snapshot
//!   older than the apply proves nothing about the applied state);
//! - the option quantity and the underlying quantity in the local ledger equal
//!   the broker's (an absent position is zero on both sides).
//!
//! - the CASH consequence agrees (see [`lifecycle_cash_agreement`]).
//!
//! Cash: the broker's account cash cannot be compared with the local ledger's
//! (the ledger's cash is seeded from configured initial equity, and the provider
//! posts the settlement cash BEFORE its activity record can ever reach this
//! ledger, so a cash delta across snapshots would not contain it). The
//! authenticated broker truth for the event's cash is the provider's own
//! settlement record. Reconciliation therefore requires all of: the journal's
//! cash, the cash the LOCAL LEDGER actually applied, and the cash re-derived at
//! reconcile time from the durable provider rows through the single settlement
//! verification, to be equal. An expiration must carry and apply zero cash. Any
//! missing or unequal source keeps the event `APPLIED_AWAITING_BROKER`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::option_lifecycle_activity::{
    fetch_option_lifecycle_activity, OptionLifecycleActivityType,
};
use mqk_db::{
    fetch_lifecycle_journal_entry, list_awaiting_broker_lifecycle_events,
    mark_lifecycle_event_reconciled, BrokerAccountAuthority, LifecycleJournalEntry,
};
use mqk_portfolio::{LedgerEntry, PortfolioState, QtyMicros};

use super::option_lifecycle_correlation::verify_lifecycle_settlement;
use super::option_lifecycle_ingestion::OPTION_LIFECYCLE_EXECUTION_DOMAIN;

fn qty_of(map: &BTreeMap<String, QtyMicros>, symbol: &str) -> QtyMicros {
    map.get(symbol).copied().unwrap_or(QtyMicros::ZERO)
}

/// Pure: do local and broker agree on both the option contract and its
/// underlying (an absent position is zero)?
pub fn lifecycle_broker_agreement(
    entry: &LifecycleJournalEntry,
    local: &BTreeMap<String, QtyMicros>,
    broker: &BTreeMap<String, QtyMicros>,
) -> bool {
    qty_of(local, &entry.option_symbol) == qty_of(broker, &entry.option_symbol)
        && qty_of(local, &entry.underlying_symbol) == qty_of(broker, &entry.underlying_symbol)
}

/// Net signed quantity per symbol of a portfolio (flat positions omitted).
pub fn local_position_quantities(portfolio: &PortfolioState) -> BTreeMap<String, QtyMicros> {
    portfolio
        .positions
        .iter()
        .map(|(symbol, position)| (symbol.clone(), position.qty_signed()))
        .filter(|(_, q)| !q.is_zero())
        .collect()
}

/// The cash each lifecycle adjustment actually applied in this portfolio's
/// ledger, by `economic_apply_id` (read from the ledger entries themselves, not
/// from any bookkeeping set).
pub fn local_lifecycle_cash(portfolio: &PortfolioState) -> BTreeMap<String, i64> {
    portfolio
        .ledger
        .iter()
        .filter_map(|e| match e {
            LedgerEntry::LifecycleAdjustment(a) => {
                Some((a.economic_apply_id.clone(), a.cash_delta_micros))
            }
            _ => None,
        })
        .collect()
}

/// The cash consequence the durable provider rows prove for `entry`, re-derived
/// now through the single settlement verification: the settlement trade's
/// signed `net_amount` for an exercise/assignment, zero for an expiration.
/// `None` when the raw evidence is missing or no longer verifies.
pub async fn provider_settlement_cash(
    pool: &PgPool,
    entry: &LifecycleJournalEntry,
) -> anyhow::Result<Option<i64>> {
    let Some(lifecycle) = fetch_option_lifecycle_activity(
        pool,
        &entry.broker_account_id,
        &entry.lifecycle_activity_id,
        entry.lifecycle_activity_type,
    )
    .await?
    else {
        return Ok(None);
    };
    let optrd = match &entry.optrd_activity_id {
        Some(id) => {
            let row = fetch_option_lifecycle_activity(
                pool,
                &entry.broker_account_id,
                id,
                OptionLifecycleActivityType::PairedTrade,
            )
            .await?;
            if row.is_none() {
                return Ok(None);
            }
            row
        }
        None => None,
    };
    Ok(verify_lifecycle_settlement(&lifecycle, optrd.as_ref())
        .ok()
        .map(|v| v.settlement.map_or(0, |s| s.cash_delta_micros)))
}

/// Pure: do the journal, the local ledger and the provider evidence all name the
/// SAME cash consequence? Any absent source is disagreement.
pub fn lifecycle_cash_agreement(
    entry: &LifecycleJournalEntry,
    local_applied_cash: Option<i64>,
    provider_cash: Option<i64>,
) -> bool {
    let journal = entry.cash_delta_micros.unwrap_or(0);
    local_applied_cash == Some(journal) && provider_cash == Some(journal)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleReconcileReport {
    pub considered: usize,
    pub reconciled: usize,
    /// The local ledger has not yet absorbed the entry.
    pub waiting_for_absorb: usize,
    /// The broker snapshot is not newer than the apply.
    pub waiting_for_fresh_snapshot: usize,
    /// Local and broker disagree on the option or its underlying.
    pub disagreeing: usize,
    /// Positions agree but the cash consequence does not (or is unproven).
    pub cash_disagreeing: usize,
}

/// Try to move every `APPLIED_AWAITING_BROKER` event of exactly `account` to
/// `RECONCILED`. Never regresses anything; an event that does not
/// qualify simply stays fenced.
pub async fn reconcile_awaiting_lifecycle_events(
    pool: &PgPool,
    account: &BrokerAccountAuthority,
    absorbed: &BTreeSet<String>,
    local_applied_cash: &BTreeMap<String, i64>,
    local: &BTreeMap<String, QtyMicros>,
    broker: &BTreeMap<String, QtyMicros>,
    broker_captured_at_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<LifecycleReconcileReport> {
    let mut report = LifecycleReconcileReport::default();
    let events = list_awaiting_broker_lifecycle_events(
        pool,
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
        account,
    )
    .await?;
    for event in events {
        report.considered += 1;
        let Some(apply_id) = event.economic_apply_id.as_deref() else {
            anyhow::bail!(
                "reconcile_awaiting_lifecycle_events: awaiting event {:?} has no economic_apply_id",
                event.lifecycle_activity_id
            );
        };
        let Some(entry) = fetch_lifecycle_journal_entry(pool, apply_id).await? else {
            anyhow::bail!("reconcile_awaiting_lifecycle_events: no journal row for {apply_id}");
        };
        if !absorbed.contains(&entry.economic_apply_id) {
            report.waiting_for_absorb += 1;
            continue;
        }
        if broker_captured_at_utc <= entry.applied_at_utc {
            report.waiting_for_fresh_snapshot += 1;
            continue;
        }
        if !lifecycle_broker_agreement(&entry, local, broker) {
            report.disagreeing += 1;
            continue;
        }
        let provider_cash = provider_settlement_cash(pool, &entry).await?;
        if !lifecycle_cash_agreement(
            &entry,
            local_applied_cash.get(&entry.economic_apply_id).copied(),
            provider_cash,
        ) {
            report.cash_disagreeing += 1;
            continue;
        }
        if mark_lifecycle_event_reconciled(
            pool,
            &event.broker_account_id,
            &event.lifecycle_activity_id,
            event.lifecycle_activity_type,
            now_utc,
        )
        .await?
        {
            report.reconciled += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_db::option_lifecycle_activity::OptionLifecycleActivityType;
    use mqk_db::CorrelationBasis;

    fn entry() -> LifecycleJournalEntry {
        LifecycleJournalEntry {
            journal_seq: 1,
            economic_apply_id: "id".to_string(),
            broker_account_id: "alpaca:a".to_string(),
            execution_domain: "equity_nyse".to_string(),
            lifecycle_activity_id: "X1".to_string(),
            lifecycle_activity_type: OptionLifecycleActivityType::Exercise,
            option_symbol: "AAPL230721C00150000".to_string(),
            underlying_symbol: "AAPL".to_string(),
            option_qty_delta_micros: -2_000_000,
            underlying_qty_delta_micros: Some(200_000_000),
            strike_micros: Some(150_000_000),
            cash_delta_micros: Some(-30_000_000_000),
            optrd_activity_id: Some("X1".to_string()),
            correlation_basis: CorrelationBasis::SameActivityId,
            applied_at_utc: Utc::now(),
            baseline_subsumed_at_utc: None,
        }
    }

    fn book(entries: &[(&str, i64)]) -> BTreeMap<String, QtyMicros> {
        entries
            .iter()
            .map(|(s, n)| (s.to_string(), QtyMicros::from_whole_units(*n).unwrap()))
            .collect()
    }

    #[test]
    fn cash_agreement_requires_journal_local_and_provider_to_name_the_same_cash() {
        let e = entry();
        let c = -30_000_000_000;
        assert!(lifecycle_cash_agreement(&e, Some(c), Some(c)));
        assert!(!lifecycle_cash_agreement(&e, Some(c + 1), Some(c)), "local");
        assert!(
            !lifecycle_cash_agreement(&e, Some(c), Some(c + 1)),
            "provider"
        );
        assert!(!lifecycle_cash_agreement(&e, None, Some(c)), "not applied");
        assert!(!lifecycle_cash_agreement(&e, Some(c), None), "no evidence");
        // An expiration carries no cash: both sides must positively say zero.
        let mut x = entry();
        x.cash_delta_micros = None;
        assert!(lifecycle_cash_agreement(&x, Some(0), Some(0)));
        assert!(!lifecycle_cash_agreement(&x, Some(5), Some(0)));
        assert!(!lifecycle_cash_agreement(&x, None, Some(0)));
    }

    #[test]
    fn agreement_requires_both_option_and_underlying_to_match_with_absent_as_zero() {
        let e = entry();
        // Post-exercise: option gone on both sides, 200 shares on both sides.
        assert!(lifecycle_broker_agreement(
            &e,
            &book(&[("AAPL", 200)]),
            &book(&[("AAPL", 200)])
        ));
        // Broker still holds the option.
        assert!(!lifecycle_broker_agreement(
            &e,
            &book(&[("AAPL", 200)]),
            &book(&[("AAPL", 200), ("AAPL230721C00150000", 2)])
        ));
        // Underlying differs.
        assert!(!lifecycle_broker_agreement(
            &e,
            &book(&[("AAPL", 200)]),
            &book(&[("AAPL", 100)])
        ));
        // Local has not absorbed (still holds the option, no shares).
        assert!(!lifecycle_broker_agreement(
            &e,
            &book(&[("AAPL230721C00150000", 2)]),
            &book(&[("AAPL", 200)])
        ));
        // Unrelated drift is not this gate's concern.
        assert!(lifecycle_broker_agreement(
            &e,
            &book(&[("AAPL", 200), ("MSFT", 5)]),
            &book(&[("AAPL", 200)])
        ));
    }
}
