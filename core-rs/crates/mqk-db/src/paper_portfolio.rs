//! Durable paper portfolio store (DURABLE-PAPER-PORTFOLIO-AND-PNL-01B).
//!
//! Two independent durable surfaces:
//!
//! - `sys_paper_portfolio_snapshots` / `sys_paper_portfolio_snapshot_positions`:
//!   an append-only, deterministically-identified copy of an accepted
//!   authoritative broker account/position snapshot. `source` distinguishes
//!   real Alpaca truth (`"external_alpaca"`) from a synthesized/diagnostic
//!   snapshot (`"synthetic_diagnostic"`) — callers must never treat the
//!   latter as authoritative Paper+Alpaca portfolio truth. This module
//!   enforces nothing about *which* snapshots are accepted; that source-
//!   authority decision belongs to the caller (mqk-daemon, wired in B4-C).
//!
//! - `sys_paper_portfolio_accounting_state`: one durable row per `run_id`,
//!   caching the result of replaying that run's already-durable, already-
//!   ordered, already-deduped `oms_inbox` applied fills through
//!   `mqk-portfolio`'s FIFO engine. This is **not** a second fill ledger —
//!   no fill content (symbol/side/qty/price) is stored here, only the
//!   computed cash/realized-P&L/fees summary and the `last_applied_inbox_id`
//!   watermark that makes re-application idempotent. The replay computation
//!   itself (reading `oms_inbox` via [`crate::inbox::inbox_load_all_applied_for_run`]
//!   and driving `mqk_portfolio::apply_fill`/`recompute_from_ledger`) lives
//!   in mqk-daemon (B4-D), which already owns the `oms_inbox` message JSON →
//!   `mqk_portfolio::Fill` conversion (`broker_event_to_portfolio_fill`).
//!   This module is the persistence layer only.
//!
//! # Idempotency
//!
//! A snapshot is idempotent by `snapshot_id` (deterministic, caller-derived —
//! see the B4-A contract for the UUIDv5 seed convention): re-inserting the
//! same `snapshot_id` with identical content is a no-op confirmation
//! ([`InsertPaperPortfolioSnapshotOutcome::AlreadyExists`]); re-inserting the
//! same `snapshot_id` with *different* content is a typed conflict
//! ([`InsertPaperPortfolioSnapshotOutcome::Conflict`]) — fail closed, never
//! silently overwritten.
//!
//! The accounting-state row is idempotent by its `last_applied_inbox_id`
//! watermark: confirming the same watermark again is a no-op
//! ([`UpsertPaperPortfolioAccountingStateOutcome::AlreadyCurrent`]); moving
//! the watermark backward is rejected
//! ([`UpsertPaperPortfolioAccountingStateOutcome::Rejected`]) rather than
//! silently accepted, since a caller computing from a stale watermark
//! indicates a replay-ordering bug, not a legitimate state.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

pub const PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA: &str = "external_alpaca";
pub const PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC: &str = "synthetic_diagnostic";

fn validate_snapshot_source(source: &str) -> Result<()> {
    match source {
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA
        | PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC => Ok(()),
        other => anyhow::bail!("invalid paper portfolio snapshot source: {other}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaperPortfolioSnapshotPosition {
    pub symbol: String,
    pub qty_signed: i64,
    pub avg_entry_price_micros: i64,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaperPortfolioSnapshotRecord {
    pub snapshot_id: Uuid,
    pub captured_at_utc: DateTime<Utc>,
    pub deployment_mode: String,
    pub source: String,
    pub equity_micros: i64,
    pub cash_micros: i64,
    pub currency: String,
    pub truth_state: String,
    pub run_id: Option<Uuid>,
    pub operation_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaperPortfolioSnapshotWithPositions {
    pub snapshot: PaperPortfolioSnapshotRecord,
    pub positions: Vec<PaperPortfolioSnapshotPosition>,
}

/// Arguments to [`insert_or_confirm_paper_portfolio_snapshot`]. Caller
/// supplies every timestamp and identity — no `DEFAULT now()` / `DEFAULT
/// gen_random_uuid()` in the schema (DB rules).
#[derive(Debug, Clone)]
pub struct NewPaperPortfolioSnapshot {
    pub snapshot_id: Uuid,
    pub captured_at_utc: DateTime<Utc>,
    pub deployment_mode: String,
    pub source: String,
    pub equity_micros: i64,
    pub cash_micros: i64,
    pub currency: String,
    pub truth_state: String,
    pub run_id: Option<Uuid>,
    pub operation_id: Option<Uuid>,
    pub positions: Vec<PaperPortfolioSnapshotPosition>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertPaperPortfolioSnapshotOutcome {
    /// A new row (and its position children) were inserted.
    Inserted {
        record: PaperPortfolioSnapshotRecord,
        positions_inserted: usize,
    },
    /// `snapshot_id` already existed with identical scalar fields and an
    /// identical (order-independent) position set — idempotent replay,
    /// zero writes.
    AlreadyExists {
        record: PaperPortfolioSnapshotRecord,
    },
    /// `snapshot_id` already existed with *different* content. Fail closed:
    /// the existing row is never overwritten.
    Conflict { detail: String },
}

fn positions_content_key(
    positions: &[PaperPortfolioSnapshotPosition],
) -> Vec<(String, i64, i64, String)> {
    let mut key: Vec<(String, i64, i64, String)> = positions
        .iter()
        .map(|p| {
            (
                p.symbol.clone(),
                p.qty_signed,
                p.avg_entry_price_micros,
                p.provenance.clone(),
            )
        })
        .collect();
    key.sort();
    key
}

/// Insert a new authoritative paper-portfolio snapshot (account fields plus
/// position children), or confirm an exact idempotent replay of one that
/// already exists. Snapshot row and position rows are inserted atomically
/// in a single transaction.
pub async fn insert_or_confirm_paper_portfolio_snapshot(
    pool: &PgPool,
    snapshot: NewPaperPortfolioSnapshot,
) -> Result<InsertPaperPortfolioSnapshotOutcome> {
    validate_snapshot_source(&snapshot.source)?;

    let mut tx = pool
        .begin()
        .await
        .context("insert_or_confirm_paper_portfolio_snapshot: begin failed")?;

    let existing = fetch_snapshot_with_positions_tx(&mut tx, snapshot.snapshot_id).await?;

    if let Some(existing) = existing {
        let candidate_record = PaperPortfolioSnapshotRecord {
            snapshot_id: snapshot.snapshot_id,
            captured_at_utc: snapshot.captured_at_utc,
            deployment_mode: snapshot.deployment_mode.clone(),
            source: snapshot.source.clone(),
            equity_micros: snapshot.equity_micros,
            cash_micros: snapshot.cash_micros,
            currency: snapshot.currency.clone(),
            truth_state: snapshot.truth_state.clone(),
            run_id: snapshot.run_id,
            operation_id: snapshot.operation_id,
        };
        let same_scalar = existing.snapshot == candidate_record;
        let same_positions = positions_content_key(&existing.positions)
            == positions_content_key(&snapshot.positions);

        tx.rollback().await.context(
            "insert_or_confirm_paper_portfolio_snapshot: rollback (read-only path) failed",
        )?;

        return if same_scalar && same_positions {
            Ok(InsertPaperPortfolioSnapshotOutcome::AlreadyExists {
                record: existing.snapshot,
            })
        } else {
            Ok(InsertPaperPortfolioSnapshotOutcome::Conflict {
                detail: format!(
                    "snapshot_id {} already exists with different content (same_scalar={same_scalar}, same_positions={same_positions})",
                    snapshot.snapshot_id
                ),
            })
        };
    }

    sqlx::query(
        r#"
        insert into sys_paper_portfolio_snapshots
            (snapshot_id, captured_at_utc, deployment_mode, source,
             equity_micros, cash_micros, currency, truth_state, run_id, operation_id)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(snapshot.snapshot_id)
    .bind(snapshot.captured_at_utc)
    .bind(&snapshot.deployment_mode)
    .bind(&snapshot.source)
    .bind(snapshot.equity_micros)
    .bind(snapshot.cash_micros)
    .bind(&snapshot.currency)
    .bind(&snapshot.truth_state)
    .bind(snapshot.run_id)
    .bind(snapshot.operation_id)
    .execute(&mut *tx)
    .await
    .context("insert_or_confirm_paper_portfolio_snapshot: insert snapshot row failed")?;

    for position in &snapshot.positions {
        sqlx::query(
            r#"
            insert into sys_paper_portfolio_snapshot_positions
                (snapshot_id, symbol, qty_signed, avg_entry_price_micros, provenance)
            values ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(snapshot.snapshot_id)
        .bind(&position.symbol)
        .bind(position.qty_signed)
        .bind(position.avg_entry_price_micros)
        .bind(&position.provenance)
        .execute(&mut *tx)
        .await
        .context("insert_or_confirm_paper_portfolio_snapshot: insert position row failed")?;
    }

    tx.commit()
        .await
        .context("insert_or_confirm_paper_portfolio_snapshot: commit failed")?;

    Ok(InsertPaperPortfolioSnapshotOutcome::Inserted {
        record: PaperPortfolioSnapshotRecord {
            snapshot_id: snapshot.snapshot_id,
            captured_at_utc: snapshot.captured_at_utc,
            deployment_mode: snapshot.deployment_mode,
            source: snapshot.source,
            equity_micros: snapshot.equity_micros,
            cash_micros: snapshot.cash_micros,
            currency: snapshot.currency,
            truth_state: snapshot.truth_state,
            run_id: snapshot.run_id,
            operation_id: snapshot.operation_id,
        },
        positions_inserted: snapshot.positions.len(),
    })
}

async fn fetch_snapshot_row(
    executor: impl sqlx::PgExecutor<'_>,
    snapshot_id: Uuid,
) -> Result<Option<PaperPortfolioSnapshotRecord>> {
    let row = sqlx::query(
        r#"
        select snapshot_id, captured_at_utc, deployment_mode, source,
               equity_micros, cash_micros, currency, truth_state, run_id, operation_id
        from sys_paper_portfolio_snapshots
        where snapshot_id = $1
        "#,
    )
    .bind(snapshot_id)
    .fetch_optional(executor)
    .await
    .context("fetch_snapshot_row failed")?;

    Ok(row.map(|r| PaperPortfolioSnapshotRecord {
        snapshot_id: r.get("snapshot_id"),
        captured_at_utc: r.get("captured_at_utc"),
        deployment_mode: r.get("deployment_mode"),
        source: r.get("source"),
        equity_micros: r.get("equity_micros"),
        cash_micros: r.get("cash_micros"),
        currency: r.get("currency"),
        truth_state: r.get("truth_state"),
        run_id: r.get("run_id"),
        operation_id: r.get("operation_id"),
    }))
}

async fn fetch_positions_rows(
    executor: impl sqlx::PgExecutor<'_>,
    snapshot_id: Uuid,
) -> Result<Vec<PaperPortfolioSnapshotPosition>> {
    let rows = sqlx::query(
        r#"
        select symbol, qty_signed, avg_entry_price_micros, provenance
        from sys_paper_portfolio_snapshot_positions
        where snapshot_id = $1
        order by symbol asc
        "#,
    )
    .bind(snapshot_id)
    .fetch_all(executor)
    .await
    .context("fetch_positions_rows failed")?;

    Ok(rows
        .into_iter()
        .map(|r| PaperPortfolioSnapshotPosition {
            symbol: r.get("symbol"),
            qty_signed: r.get("qty_signed"),
            avg_entry_price_micros: r.get("avg_entry_price_micros"),
            provenance: r.get("provenance"),
        })
        .collect())
}

async fn fetch_snapshot_with_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot_id: Uuid,
) -> Result<Option<PaperPortfolioSnapshotWithPositions>> {
    let Some(snapshot) = fetch_snapshot_row(&mut **tx, snapshot_id).await? else {
        return Ok(None);
    };
    let positions = fetch_positions_rows(&mut **tx, snapshot_id).await?;
    Ok(Some(PaperPortfolioSnapshotWithPositions {
        snapshot,
        positions,
    }))
}

/// Fetch one snapshot (with its position children) by exact `snapshot_id`.
/// Returns `None` when absent — never fabricates or substitutes a nearby
/// snapshot.
pub async fn fetch_paper_portfolio_snapshot_by_id(
    pool: &PgPool,
    snapshot_id: Uuid,
) -> Result<Option<PaperPortfolioSnapshotWithPositions>> {
    let Some(snapshot) = fetch_snapshot_row(pool, snapshot_id).await? else {
        return Ok(None);
    };
    let positions = fetch_positions_rows(pool, snapshot_id).await?;
    Ok(Some(PaperPortfolioSnapshotWithPositions {
        snapshot,
        positions,
    }))
}

/// Fetch the most recently captured snapshot (with positions) for a given
/// `deployment_mode` + `source`, or `None` if none exists yet. Callers
/// wanting only authoritative Paper+Alpaca truth must pass
/// `source = PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA`.
pub async fn fetch_latest_paper_portfolio_snapshot(
    pool: &PgPool,
    deployment_mode: &str,
    source: &str,
) -> Result<Option<PaperPortfolioSnapshotWithPositions>> {
    let row = sqlx::query(
        r#"
        select snapshot_id, captured_at_utc, deployment_mode, source,
               equity_micros, cash_micros, currency, truth_state, run_id, operation_id
        from sys_paper_portfolio_snapshots
        where deployment_mode = $1 and source = $2
        order by captured_at_utc desc, snapshot_id desc
        limit 1
        "#,
    )
    .bind(deployment_mode)
    .bind(source)
    .fetch_optional(pool)
    .await
    .context("fetch_latest_paper_portfolio_snapshot failed")?;

    let Some(row) = row else {
        return Ok(None);
    };
    let snapshot_id: Uuid = row.get("snapshot_id");
    let snapshot = PaperPortfolioSnapshotRecord {
        snapshot_id,
        captured_at_utc: row.get("captured_at_utc"),
        deployment_mode: row.get("deployment_mode"),
        source: row.get("source"),
        equity_micros: row.get("equity_micros"),
        cash_micros: row.get("cash_micros"),
        currency: row.get("currency"),
        truth_state: row.get("truth_state"),
        run_id: row.get("run_id"),
        operation_id: row.get("operation_id"),
    };
    let positions = fetch_positions_rows(pool, snapshot_id).await?;
    Ok(Some(PaperPortfolioSnapshotWithPositions {
        snapshot,
        positions,
    }))
}

/// Fetch up to `limit` most recent snapshots (scalar fields only, no
/// position children) for a given `deployment_mode` + `source`, newest
/// first.
pub async fn fetch_recent_paper_portfolio_snapshots(
    pool: &PgPool,
    deployment_mode: &str,
    source: &str,
    limit: i64,
) -> Result<Vec<PaperPortfolioSnapshotRecord>> {
    if limit <= 0 {
        anyhow::bail!("fetch_recent_paper_portfolio_snapshots: limit must be positive");
    }
    let rows = sqlx::query(
        r#"
        select snapshot_id, captured_at_utc, deployment_mode, source,
               equity_micros, cash_micros, currency, truth_state, run_id, operation_id
        from sys_paper_portfolio_snapshots
        where deployment_mode = $1 and source = $2
        order by captured_at_utc desc, snapshot_id desc
        limit $3
        "#,
    )
    .bind(deployment_mode)
    .bind(source)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_paper_portfolio_snapshots failed")?;

    Ok(rows
        .into_iter()
        .map(|r| PaperPortfolioSnapshotRecord {
            snapshot_id: r.get("snapshot_id"),
            captured_at_utc: r.get("captured_at_utc"),
            deployment_mode: r.get("deployment_mode"),
            source: r.get("source"),
            equity_micros: r.get("equity_micros"),
            cash_micros: r.get("cash_micros"),
            currency: r.get("currency"),
            truth_state: r.get("truth_state"),
            run_id: r.get("run_id"),
            operation_id: r.get("operation_id"),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Accounting state
// ---------------------------------------------------------------------------

pub const PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE: &str = "complete";
pub const PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE: &str = "incomplete";

fn validate_accounting_epoch(epoch: &str) -> Result<()> {
    match epoch {
        PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE | PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE => {
            Ok(())
        }
        other => anyhow::bail!("invalid paper portfolio accounting epoch: {other}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaperPortfolioAccountingStateRecord {
    pub run_id: Uuid,
    pub cash_micros: i64,
    pub realized_pnl_micros: i64,
    pub fees_micros: i64,
    pub last_applied_inbox_id: i64,
    pub accounting_epoch: String,
    pub accounting_epoch_reason: Option<String>,
    pub updated_at_utc: DateTime<Utc>,
}

/// Arguments to [`upsert_paper_portfolio_accounting_state`]. `last_applied_inbox_id`
/// is the caller's freshly-computed replay watermark (from
/// `mqk_db::inbox_load_all_applied_for_run`'s highest `inbox_id`, or 0 if no
/// applied fills exist yet).
#[derive(Debug, Clone)]
pub struct UpsertPaperPortfolioAccountingStateArgs {
    pub run_id: Uuid,
    pub cash_micros: i64,
    pub realized_pnl_micros: i64,
    pub fees_micros: i64,
    pub last_applied_inbox_id: i64,
    pub accounting_epoch: String,
    pub accounting_epoch_reason: Option<String>,
    pub updated_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpsertPaperPortfolioAccountingStateOutcome {
    /// No row existed yet for this `run_id` — inserted.
    Inserted {
        record: PaperPortfolioAccountingStateRecord,
    },
    /// A row existed with a strictly lower `last_applied_inbox_id` —
    /// updated to the new watermark/summary.
    Updated {
        previous_last_applied_inbox_id: i64,
        record: PaperPortfolioAccountingStateRecord,
    },
    /// A row already existed with exactly this `last_applied_inbox_id` —
    /// idempotent confirmation, zero writes. The existing row is returned
    /// unchanged (the caller's freshly-recomputed values are not compared
    /// or written, since a deterministic replay of the same inbox rows
    /// always yields the same result by construction).
    AlreadyCurrent {
        record: PaperPortfolioAccountingStateRecord,
    },
    /// A row already existed with a *higher* `last_applied_inbox_id` than
    /// the caller supplied. Fail closed: the caller computed from a stale
    /// watermark (a replay-ordering bug), so the existing row is never
    /// regressed.
    Rejected {
        current_last_applied_inbox_id: i64,
        detail: String,
    },
}

/// Insert or confirm the durable accounting-state row for one `run_id`.
pub async fn upsert_paper_portfolio_accounting_state(
    pool: &PgPool,
    args: UpsertPaperPortfolioAccountingStateArgs,
) -> Result<UpsertPaperPortfolioAccountingStateOutcome> {
    validate_accounting_epoch(&args.accounting_epoch)?;
    if args.last_applied_inbox_id < 0 {
        anyhow::bail!(
            "upsert_paper_portfolio_accounting_state: last_applied_inbox_id must be >= 0"
        );
    }

    let mut tx = pool
        .begin()
        .await
        .context("upsert_paper_portfolio_accounting_state: begin failed")?;

    let existing = sqlx::query(
        r#"
        select run_id, cash_micros, realized_pnl_micros, fees_micros,
               last_applied_inbox_id, accounting_epoch, accounting_epoch_reason, updated_at_utc
        from sys_paper_portfolio_accounting_state
        where run_id = $1
        for update
        "#,
    )
    .bind(args.run_id)
    .fetch_optional(&mut *tx)
    .await
    .context("upsert_paper_portfolio_accounting_state: select-for-update failed")?;

    let new_record = PaperPortfolioAccountingStateRecord {
        run_id: args.run_id,
        cash_micros: args.cash_micros,
        realized_pnl_micros: args.realized_pnl_micros,
        fees_micros: args.fees_micros,
        last_applied_inbox_id: args.last_applied_inbox_id,
        accounting_epoch: args.accounting_epoch.clone(),
        accounting_epoch_reason: args.accounting_epoch_reason.clone(),
        updated_at_utc: args.updated_at_utc,
    };

    match existing {
        None => {
            sqlx::query(
                r#"
                insert into sys_paper_portfolio_accounting_state
                    (run_id, cash_micros, realized_pnl_micros, fees_micros,
                     last_applied_inbox_id, accounting_epoch, accounting_epoch_reason, updated_at_utc)
                values ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(args.run_id)
            .bind(args.cash_micros)
            .bind(args.realized_pnl_micros)
            .bind(args.fees_micros)
            .bind(args.last_applied_inbox_id)
            .bind(&args.accounting_epoch)
            .bind(&args.accounting_epoch_reason)
            .bind(args.updated_at_utc)
            .execute(&mut *tx)
            .await
            .context("upsert_paper_portfolio_accounting_state: insert failed")?;

            tx.commit()
                .await
                .context("upsert_paper_portfolio_accounting_state: commit (insert) failed")?;

            Ok(UpsertPaperPortfolioAccountingStateOutcome::Inserted { record: new_record })
        }
        Some(row) => {
            let existing_watermark: i64 = row.get("last_applied_inbox_id");
            if args.last_applied_inbox_id < existing_watermark {
                tx.rollback().await.context(
                    "upsert_paper_portfolio_accounting_state: rollback (rejected) failed",
                )?;
                return Ok(UpsertPaperPortfolioAccountingStateOutcome::Rejected {
                    current_last_applied_inbox_id: existing_watermark,
                    detail: format!(
                        "run_id {} already has last_applied_inbox_id {existing_watermark}, refusing to regress to {}",
                        args.run_id, args.last_applied_inbox_id
                    ),
                });
            }
            if args.last_applied_inbox_id == existing_watermark {
                let record = PaperPortfolioAccountingStateRecord {
                    run_id: row.get("run_id"),
                    cash_micros: row.get("cash_micros"),
                    realized_pnl_micros: row.get("realized_pnl_micros"),
                    fees_micros: row.get("fees_micros"),
                    last_applied_inbox_id: row.get("last_applied_inbox_id"),
                    accounting_epoch: row.get("accounting_epoch"),
                    accounting_epoch_reason: row.get("accounting_epoch_reason"),
                    updated_at_utc: row.get("updated_at_utc"),
                };
                tx.rollback().await.context(
                    "upsert_paper_portfolio_accounting_state: rollback (already-current) failed",
                )?;
                return Ok(UpsertPaperPortfolioAccountingStateOutcome::AlreadyCurrent { record });
            }

            sqlx::query(
                r#"
                update sys_paper_portfolio_accounting_state
                set cash_micros = $2,
                    realized_pnl_micros = $3,
                    fees_micros = $4,
                    last_applied_inbox_id = $5,
                    accounting_epoch = $6,
                    accounting_epoch_reason = $7,
                    updated_at_utc = $8
                where run_id = $1
                "#,
            )
            .bind(args.run_id)
            .bind(args.cash_micros)
            .bind(args.realized_pnl_micros)
            .bind(args.fees_micros)
            .bind(args.last_applied_inbox_id)
            .bind(&args.accounting_epoch)
            .bind(&args.accounting_epoch_reason)
            .bind(args.updated_at_utc)
            .execute(&mut *tx)
            .await
            .context("upsert_paper_portfolio_accounting_state: update failed")?;

            tx.commit()
                .await
                .context("upsert_paper_portfolio_accounting_state: commit (update) failed")?;

            Ok(UpsertPaperPortfolioAccountingStateOutcome::Updated {
                previous_last_applied_inbox_id: existing_watermark,
                record: new_record,
            })
        }
    }
}

/// Fetch the durable accounting-state row for one `run_id`. Returns `None`
/// when no fills have ever been applied for this run — never fabricates a
/// zeroed row.
pub async fn fetch_paper_portfolio_accounting_state(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<PaperPortfolioAccountingStateRecord>> {
    let row = sqlx::query(
        r#"
        select run_id, cash_micros, realized_pnl_micros, fees_micros,
               last_applied_inbox_id, accounting_epoch, accounting_epoch_reason, updated_at_utc
        from sys_paper_portfolio_accounting_state
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .context("fetch_paper_portfolio_accounting_state failed")?;

    Ok(row.map(|r| PaperPortfolioAccountingStateRecord {
        run_id: r.get("run_id"),
        cash_micros: r.get("cash_micros"),
        realized_pnl_micros: r.get("realized_pnl_micros"),
        fees_micros: r.get("fees_micros"),
        last_applied_inbox_id: r.get("last_applied_inbox_id"),
        accounting_epoch: r.get("accounting_epoch"),
        accounting_epoch_reason: r.get("accounting_epoch_reason"),
        updated_at_utc: r.get("updated_at_utc"),
    }))
}
