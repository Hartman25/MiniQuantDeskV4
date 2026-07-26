//! Durable fill accounting and P&L truth (DURABLE-PAPER-PORTFOLIO-AND-PNL-01D).
//!
//! Replays a run's already-durable, already-ordered, already-deduped
//! `oms_inbox` applied fills through `mqk-portfolio`'s FIFO engine (reusing
//! [`super::snapshot::recover_oms_and_portfolio`] directly — that function
//! already applies the exact same duplicate-fill guard the live apply path
//! uses, keyed on OMS per-order applied-event-identity, not just raw inbox
//! row iteration; reimplementing a second, simpler replay here would risk
//! silently double-counting a fill that arrived via both WS and REST with
//! different `broker_message_id`s but the same economic identity), then
//! persists the computed summary via
//! [`mqk_db::upsert_paper_portfolio_accounting_state`].
//!
//! `cash_micros` here is the *cumulative cash movement* produced by this
//! run's fills (computed with `initial_cash_micros = 0`), not the absolute
//! account cash balance — that already lives on the durable snapshot
//! (B4-C). `realized_pnl_micros` and the position lots are read directly
//! off the replayed `PortfolioState`; `fees_micros` is summed from the
//! ledger's `Fill` entries (not tracked as a separate running total inside
//! `mqk-portfolio` itself).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Legacy single-mismatch reason prefix, retained only as a doc/historical
/// reference — [`replay_paper_portfolio_accounting`] now emits one of the
/// bidirectional reason codes below (`broker_position_missing_fill_history`,
/// `fill_history_position_missing_at_broker`, `position_quantity_mismatch`,
/// `broker_position_quantity_unparseable`, `duplicate_broker_position_symbol`).
#[allow(dead_code)]
pub(crate) const ACCOUNTING_EPOCH_INCOMPLETE_REASON_PREFIX: &str =
    "pre_existing_position_no_matching_fill_history";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PaperAccountingReplay {
    pub cash_micros: i64,
    pub realized_pnl_micros: i64,
    pub fees_micros: i64,
    pub last_applied_inbox_id: i64,
    pub accounting_epoch: &'static str,
    pub accounting_epoch_reason: Option<String>,
}

/// Groups raw broker positions by symbol, parses quantities, and reports
/// every validation failure as a deterministic bounded reason code instead
/// of silently skipping it. A symbol that appears more than once in
/// `broker_positions` (whether or not the repeated rows agree) is itself an
/// anomaly and is reported as `duplicate_broker_position_symbol` rather than
/// merged or de-duplicated silently.
///
/// Returns `(symbol -> nonzero signed qty, sorted blocker reason codes)`.
fn normalize_broker_positions(
    broker_positions: &[mqk_schemas::BrokerPosition],
) -> (BTreeMap<String, i64>, Vec<String>) {
    let mut by_symbol: BTreeMap<String, Vec<Option<i64>>> = BTreeMap::new();
    let mut blockers = Vec::new();
    for bp in broker_positions {
        let symbol = bp.symbol.trim();
        if symbol.is_empty() {
            // A2: a blank broker position symbol must never be silently
            // skipped -- it is bounded, deterministic completeness evidence
            // in its own right, not an absence of evidence.
            blockers.push("broker_position_symbol_blank".to_string());
            continue;
        }
        let parsed = super::snapshot::parse_signed_qty(&bp.qty);
        by_symbol
            .entry(symbol.to_string())
            .or_default()
            .push(parsed);
    }

    let mut qty_by_symbol = BTreeMap::new();
    for (symbol, parses) in by_symbol {
        if parses.len() > 1 {
            blockers.push(format!("duplicate_broker_position_symbol:{symbol}"));
            continue;
        }
        match parses[0] {
            None => blockers.push(format!("broker_position_quantity_unparseable:{symbol}")),
            Some(0) => {}
            Some(qty) => {
                qty_by_symbol.insert(symbol, qty);
            }
        }
    }
    (qty_by_symbol, blockers)
}

/// Replays `run_id`'s durable fill history and cross-checks it, in both
/// directions, against `broker_positions` (the authoritative, currently-known
/// broker position truth for this run — callers should pass the same
/// positions from the broker snapshot they just accepted).
///
/// Bidirectional completeness (B4 closure repair): the prior implementation
/// only checked "does every nonzero broker position have a matching
/// fill-derived quantity", which silently accepted a fill-derived nonzero
/// position that the broker snapshot no longer reports. Both directions are
/// now checked, plus broker-side data-quality failures (unparseable
/// quantity, duplicate/conflicting symbol) that must never be silently
/// skipped past. This function never fabricates a synthetic opening fill to
/// force a match — an incomplete epoch (with every detected reason code,
/// sorted deterministically) is reported truthfully instead.
pub(crate) async fn replay_paper_portfolio_accounting(
    pool: &PgPool,
    run_id: Uuid,
    broker_positions: &[mqk_schemas::BrokerPosition],
) -> anyhow::Result<PaperAccountingReplay> {
    let applied = mqk_db::inbox_load_all_applied_for_run(pool, run_id)
        .await
        .map_err(|err| anyhow::anyhow!("inbox_load_all_applied_for_run failed: {err}"))?;
    let last_applied_inbox_id = applied.iter().map(|r| r.inbox_id).max().unwrap_or(0);

    let (_, _, portfolio) = super::snapshot::recover_oms_and_portfolio(pool, run_id, 0)
        .await
        .map_err(|err| anyhow::anyhow!("recover_oms_and_portfolio failed: {err}"))?;

    let fees_micros: i64 = portfolio
        .ledger
        .iter()
        .filter_map(|entry| match entry {
            mqk_portfolio::LedgerEntry::Fill(fill) => Some(fill.fee_micros),
            _ => None,
        })
        .sum();

    let (broker_qty_by_symbol, mut blockers) = normalize_broker_positions(broker_positions);

    let replay_qty_by_symbol: BTreeMap<String, i64> = portfolio
        .positions
        .iter()
        .filter_map(|(symbol, pos)| {
            let net: i64 = pos.lots.iter().map(|lot| lot.qty_signed).sum();
            if net == 0 {
                None
            } else {
                Some((symbol.clone(), net))
            }
        })
        .collect();

    let all_symbols: std::collections::BTreeSet<&String> = broker_qty_by_symbol
        .keys()
        .chain(replay_qty_by_symbol.keys())
        .collect();
    for symbol in all_symbols {
        match (
            broker_qty_by_symbol.get(symbol),
            replay_qty_by_symbol.get(symbol),
        ) {
            (Some(_), None) => {
                blockers.push(format!("broker_position_missing_fill_history:{symbol}"));
            }
            (None, Some(_)) => {
                blockers.push(format!("fill_history_position_missing_at_broker:{symbol}"));
            }
            (Some(broker_qty), Some(replay_qty)) if broker_qty != replay_qty => {
                blockers.push(format!("position_quantity_mismatch:{symbol}"));
            }
            _ => {}
        }
    }
    blockers.sort();
    blockers.dedup();

    let mut accounting_epoch = "complete";
    let mut accounting_epoch_reason = None;
    if !blockers.is_empty() {
        accounting_epoch = "incomplete";
        accounting_epoch_reason = Some(blockers.join(";"));
    }

    Ok(PaperAccountingReplay {
        cash_micros: portfolio.cash_micros,
        realized_pnl_micros: portfolio.realized_pnl_micros,
        fees_micros,
        last_applied_inbox_id,
        accounting_epoch,
        accounting_epoch_reason,
    })
}

/// Best-effort durable refresh: replays and upserts the accounting state
/// for `run_id`, gated the same way [`super::snapshot::persist_external_broker_snapshot_best_effort`]
/// is (Paper + Alpaca only). Every failure mode is logged and swallowed —
/// this is additive truth on top of whatever in-memory/durable snapshot
/// truth already exists, never a gate for it.
///
/// `source_snapshot_id` must be a *confirmed* durable snapshot id (from
/// [`super::snapshot::ExternalSnapshotPersistOutcome::Confirmed`]) — every
/// call site is required to gate on that confirmation before calling this
/// function (B4 closure repair, Repair C).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn refresh_paper_portfolio_accounting_state_best_effort(
    db: Option<PgPool>,
    deployment_mode: super::types::DeploymentMode,
    broker_kind: Option<super::types::BrokerKind>,
    run_id: Uuid,
    source_snapshot_id: Uuid,
    broker_positions: Vec<mqk_schemas::BrokerPosition>,
    occurred_at_utc: DateTime<Utc>,
) {
    if deployment_mode != super::types::DeploymentMode::Paper {
        return;
    }
    if broker_kind != Some(super::types::BrokerKind::Alpaca) {
        return;
    }
    let Some(pool) = db else {
        tracing::warn!("durable_paper_portfolio_accounting_skip: no_db_pool_configured");
        return;
    };

    let replay = match replay_paper_portfolio_accounting(&pool, run_id, &broker_positions).await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "durable_paper_portfolio_accounting_replay_failed");
            return;
        }
    };

    let result = mqk_db::upsert_paper_portfolio_accounting_state(
        &pool,
        mqk_db::UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: replay.cash_micros,
            realized_pnl_micros: replay.realized_pnl_micros,
            fees_micros: replay.fees_micros,
            last_applied_inbox_id: replay.last_applied_inbox_id,
            accounting_epoch: replay.accounting_epoch.to_string(),
            accounting_epoch_reason: replay.accounting_epoch_reason,
            updated_at_utc: occurred_at_utc,
            source_snapshot_id,
        },
    )
    .await;

    match result {
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. })
        | Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Updated { .. })
        | Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::UpdatedForSnapshot { .. })
        | Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::AlreadyCurrent { .. }) => {}
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Rejected { detail, .. }) => {
            // Should not happen in normal operation (the watermark only ever
            // advances) -- a stale-replay bug would surface here, so this is
            // a warning, not a silent drop.
            tracing::warn!(detail = %detail, "durable_paper_portfolio_accounting_watermark_rejected");
        }
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Conflict { detail, .. }) => {
            // Same watermark, same snapshot, but different fill-derived
            // values (or a drifted epoch/reason for the same snapshot) --
            // nondeterministic replay or corruption. Never overwritten;
            // surfaced as a warning for operator diagnosis.
            tracing::warn!(detail = %detail, "durable_paper_portfolio_accounting_conflict");
        }
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot {
            detail,
        }) => {
            // The caller supplied a source_snapshot_id that does not resolve
            // to a durable, run-scoped, paper/external_alpaca/USD snapshot --
            // should not happen since every call site here gates on a
            // Confirmed persistence outcome first, but fail closed with a
            // warning rather than silently dropping the replay.
            tracing::warn!(detail = %detail, "durable_paper_portfolio_accounting_invalid_source_snapshot");
        }
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::RejectedStaleSnapshot {
            detail,
        }) => {
            // The candidate snapshot is not newer (or not-older, on a higher
            // watermark) than the snapshot already recorded -- provenance
            // must never move backward. Never overwritten.
            tracing::warn!(detail = %detail, "durable_paper_portfolio_accounting_stale_snapshot_rejected");
        }
        Err(err) => {
            tracing::warn!(error = %err, "durable_paper_portfolio_accounting_persist_failed");
        }
    }
}

// ---------------------------------------------------------------------------
// B4 final closure repair (A2): bidirectional completeness -- blank symbols
// must never be silently skipped.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod normalize_broker_positions_tests {
    use super::normalize_broker_positions;
    use mqk_schemas::BrokerPosition;

    fn position(symbol: &str, qty: &str) -> BrokerPosition {
        BrokerPosition {
            symbol: symbol.to_string(),
            qty: qty.to_string(),
            avg_price: "150.00".to_string(),
        }
    }

    /// A2: a blank symbol is a bounded blocker, never a silent skip -- the
    /// prior implementation's `continue` on `symbol.is_empty()` dropped the
    /// row with zero trace.
    #[test]
    fn blank_symbol_produces_bounded_blocker() {
        let positions = vec![position("", "10")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert!(qty_by_symbol.is_empty());
        assert_eq!(blockers, vec!["broker_position_symbol_blank".to_string()]);
    }

    /// A2: a whitespace-only symbol is blank after trimming.
    #[test]
    fn whitespace_only_symbol_produces_bounded_blocker() {
        let positions = vec![position("   ", "10")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert!(qty_by_symbol.is_empty());
        assert_eq!(blockers, vec!["broker_position_symbol_blank".to_string()]);
    }

    /// A2: multiple blank-symbol rows collapse to one blocker (deduped by
    /// the caller's `blockers.sort(); blockers.dedup();`), and a valid
    /// position alongside a blank one is still reported.
    #[test]
    fn valid_position_alongside_blank_symbol_is_still_reported() {
        let positions = vec![position("AAPL", "10"), position("", "5")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert_eq!(qty_by_symbol.get("AAPL"), Some(&10));
        assert_eq!(blockers, vec!["broker_position_symbol_blank".to_string()]);
    }

    /// Existing behavior preserved: an unparseable quantity is a bounded
    /// blocker keyed by symbol.
    #[test]
    fn unparseable_quantity_produces_bounded_blocker() {
        let positions = vec![position("AAPL", "not-a-number")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert!(qty_by_symbol.is_empty());
        assert_eq!(
            blockers,
            vec!["broker_position_quantity_unparseable:AAPL".to_string()]
        );
    }

    /// Existing behavior preserved: a duplicate symbol is a bounded blocker,
    /// never silently merged or overwritten.
    #[test]
    fn duplicate_symbol_produces_bounded_blocker() {
        let positions = vec![position("AAPL", "10"), position("AAPL", "20")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert!(qty_by_symbol.is_empty());
        assert_eq!(
            blockers,
            vec!["duplicate_broker_position_symbol:AAPL".to_string()]
        );
    }

    /// A zero-quantity position is dropped from `qty_by_symbol` (flat, not
    /// a blocker) -- existing behavior preserved.
    #[test]
    fn zero_quantity_is_dropped_without_blocker() {
        let positions = vec![position("AAPL", "0")];
        let (qty_by_symbol, blockers) = normalize_broker_positions(&positions);
        assert!(qty_by_symbol.is_empty());
        assert!(blockers.is_empty());
    }
}
