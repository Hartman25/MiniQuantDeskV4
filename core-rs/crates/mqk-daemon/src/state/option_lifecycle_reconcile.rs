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
//! Scope note on cash: the local ledger's cash is seeded from configured
//! initial equity and moves only with recorded fills and provider-evidenced
//! adjustments, never from the broker's account cash, so broker cash cannot be
//! compared with it without inventing a baseline. The cash effect is therefore
//! verified at its evidence source instead (the settlement trade's
//! `net_amount` must equal the signed strike cash before the event is ever
//! READY -- `option_lifecycle_correlation::optrd_satisfies_facts`), and the
//! gate's broker-agreement condition is the position state reconcile itself
//! compares.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::{
    fetch_lifecycle_journal_entry, list_awaiting_broker_lifecycle_events,
    mark_lifecycle_event_reconciled, LifecycleJournalEntry,
};
use mqk_portfolio::{PortfolioState, QtyMicros};

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
}

/// Try to move every `APPLIED_AWAITING_BROKER` event of `deployment_mode`
/// accounts to `RECONCILED`. Never regresses anything; an event that does not
/// qualify simply stays fenced.
pub async fn reconcile_awaiting_lifecycle_events(
    pool: &PgPool,
    deployment_mode: &str,
    absorbed: &BTreeSet<String>,
    local: &BTreeMap<String, QtyMicros>,
    broker: &BTreeMap<String, QtyMicros>,
    broker_captured_at_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<LifecycleReconcileReport> {
    let mut report = LifecycleReconcileReport::default();
    let events = list_awaiting_broker_lifecycle_events(
        pool,
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
        deployment_mode,
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
