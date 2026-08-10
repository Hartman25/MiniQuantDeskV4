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

// ---------------------------------------------------------------------------
// PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect #3): FC-3C.
//
// `replay_paper_portfolio_accounting` already sources its economic truth
// from `super::snapshot::recover_oms_and_portfolio` directly (see the
// `Ok((_, _, portfolio)) = super::snapshot::recover_oms_and_portfolio(...)`
// call above) -- it was never a second, independent durable-replay
// implementation. This proves that wiring holds and stays a regression
// guard against it ever being decoupled: the SAME durable evidence must
// produce identical cash/realized-P&L truth from both call sites.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod fc3c_canonical_replay_parity_tests {
    use super::replay_paper_portfolio_accounting;
    use chrono::{TimeZone, Utc};
    use mqk_execution::{BrokerEvent, Side};
    use uuid::Uuid;

    fn fixed_run_id(seed: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("test.fc3c-canonical-replay-parity.v1|{seed}").as_bytes(),
        )
    }

    async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "test-fc3c-canonical-replay-parity".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc.with_ymd_and_hms(2099, 4, 1, 12, 0, 0).unwrap(),
                git_hash: "test".to_string(),
                config_hash: "test".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "test".to_string(),
            },
        )
        .await
        .expect("fixture run insert should succeed");
    }

    async fn fixture_order(pool: &sqlx::PgPool, run_id: Uuid, order_id: &str, qty: i64) {
        mqk_db::outbox_enqueue(
            pool,
            run_id,
            order_id,
            serde_json::json!({"symbol": "AAPL", "qty": qty, "side": "buy"}),
        )
        .await
        .expect("outbox_enqueue should succeed");
        sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
            .bind(order_id)
            .execute(pool)
            .await
            .expect("mark outbox SENT should succeed");
    }

    #[allow(clippy::too_many_arguments)]
    async fn fixture_applied_event(
        pool: &sqlx::PgPool,
        run_id: Uuid,
        broker_message_id: &str,
        internal_order_id: &str,
        event_kind: &str,
        event: &BrokerEvent,
        at: chrono::DateTime<Utc>,
    ) {
        let json = serde_json::to_value(event).expect("serialize BrokerEvent");
        mqk_db::inbox_insert_deduped_with_identity(
            pool,
            run_id,
            broker_message_id,
            None,
            internal_order_id,
            "broker-order-1",
            event_kind,
            &json,
            0,
            at,
        )
        .await
        .expect("inbox insert should succeed");
        mqk_db::inbox_mark_applied(pool, run_id, broker_message_id, at)
            .await
            .expect("inbox mark applied should succeed");
    }

    /// FC-3C: for the SAME run_id/durable evidence, `recover_oms_and_portfolio`
    /// (called directly) and `replay_paper_portfolio_accounting` (which calls
    /// it internally) must derive the SAME economic cash/realized-P&L truth.
    /// Uses the same terminal-overfill scenario as FC-1/FC-3B (order total=3,
    /// partial 2@$100, terminal raw delta=2@$102 -> true remainder 1) so this
    /// also re-proves Defect #1's fix is visible through this second consumer.
    #[tokio::test]
    async fn fc3c_recover_and_accounting_replay_agree_on_cash_and_pnl() {
        mqk_db::run_isolated("fc3c_canonical_replay_parity", |pool| async move {
            let run_id = fixed_run_id("fc3c_canonical_replay_parity");
            let order_id = "order-fc3c";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, 3).await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let partial = BrokerEvent::PartialFill {
                broker_message_id: format!("alpaca:{order_id}:partial_fill:ws-2"),
                broker_fill_id: None,
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 2,
                price_micros: 100_000_000,
                fee_micros: 0,
                cum_qty_after: Some(2),
            };
            fixture_applied_event(
                &pool,
                run_id,
                "alpaca:order-fc3c:partial_fill:ws-2",
                order_id,
                "partial_fill",
                &partial,
                at,
            )
            .await;

            let terminal = BrokerEvent::Fill {
                broker_message_id: format!("alpaca:{order_id}:fill:term"),
                broker_fill_id: None,
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 2,
                price_micros: 102_000_000,
                fee_micros: 0,
            };
            fixture_applied_event(
                &pool,
                run_id,
                "alpaca:order-fc3c:fill:term",
                order_id,
                "fill",
                &terminal,
                at,
            )
            .await;

            // Consumer 1: recover_oms_and_portfolio, called directly.
            let (_, _, direct) =
                super::super::snapshot::recover_oms_and_portfolio(&pool, run_id, 0)
                    .await
                    .expect("direct recover_oms_and_portfolio should succeed");

            // Consumer 2: replay_paper_portfolio_accounting, which calls
            // recover_oms_and_portfolio internally.
            let via_accounting = replay_paper_portfolio_accounting(&pool, run_id, &[])
                .await
                .expect("replay_paper_portfolio_accounting should succeed");

            assert_eq!(
                direct.cash_micros, via_accounting.cash_micros,
                "FC-3C: recover_oms_and_portfolio and replay_paper_portfolio_accounting must \
                 agree on cash_micros for identical durable evidence"
            );
            assert_eq!(
                direct.realized_pnl_micros, via_accounting.realized_pnl_micros,
                "FC-3C: recover_oms_and_portfolio and replay_paper_portfolio_accounting must \
                 agree on realized_pnl_micros for identical durable evidence"
            );
            // Independent proof this scenario is the FC-1/FC-3B terminal-
            // overfill case: cash must reflect qty=3 (2@$100 + effective
            // 1@$102), not qty=4.
            let expected_cash = -(2 * 100_000_000 + 102_000_000);
            assert_eq!(
                direct.cash_micros, expected_cash,
                "FC-3C: cash must reflect the effective 1-share terminal fill, not the raw \
                 2-share delta"
            );
        })
        .await;
    }
}
