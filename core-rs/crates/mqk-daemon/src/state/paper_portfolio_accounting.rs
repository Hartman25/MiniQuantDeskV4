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

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

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

/// Replays `run_id`'s durable fill history and cross-checks it against
/// `broker_positions` (the authoritative, currently-known broker position
/// truth for this run — callers should pass the same positions from the
/// broker snapshot they just accepted).
///
/// Any nonzero broker position whose FIFO-replayed net quantity does not
/// exactly match is flagged `incomplete`: the fill history known to this
/// run does not fully explain that position (most commonly, a pre-existing
/// broker position adopted before any fill in this run's `oms_inbox`
/// history). This function never fabricates a synthetic opening fill to
/// force a match — an incomplete epoch is reported truthfully instead.
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

    let mut accounting_epoch = "complete";
    let mut accounting_epoch_reason = None;
    for bp in broker_positions {
        let Some(broker_qty_signed) = super::snapshot::parse_signed_qty(&bp.qty) else {
            continue;
        };
        if broker_qty_signed == 0 {
            continue;
        }
        let ledger_qty_signed: i64 = portfolio
            .positions
            .get(&bp.symbol)
            .map(|p| p.lots.iter().map(|lot| lot.qty_signed).sum())
            .unwrap_or(0);
        if ledger_qty_signed != broker_qty_signed {
            accounting_epoch = "incomplete";
            accounting_epoch_reason = Some(format!(
                "{ACCOUNTING_EPOCH_INCOMPLETE_REASON_PREFIX}:{}:broker_qty={broker_qty_signed}:fill_history_derived_qty={ledger_qty_signed}",
                bp.symbol
            ));
            break;
        }
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
#[allow(clippy::too_many_arguments)]
pub(crate) async fn refresh_paper_portfolio_accounting_state_best_effort(
    db: Option<PgPool>,
    deployment_mode: super::types::DeploymentMode,
    broker_kind: Option<super::types::BrokerKind>,
    run_id: Uuid,
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
        },
    )
    .await;

    match result {
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. })
        | Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Updated { .. })
        | Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::AlreadyCurrent { .. }) => {}
        Ok(mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Rejected { detail, .. }) => {
            // Should not happen in normal operation (the watermark only ever
            // advances) -- a stale-replay bug would surface here, so this is
            // a warning, not a silent drop.
            tracing::warn!(detail = %detail, "durable_paper_portfolio_accounting_watermark_rejected");
        }
        Err(err) => {
            tracing::warn!(error = %err, "durable_paper_portfolio_accounting_persist_failed");
        }
    }
}
