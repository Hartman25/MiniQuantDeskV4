//! Durable held-quantity state for `FixedInitialCapitalFractionV1`
//! (`sys_strategy_held_sizing_state`, migration 0091).
//!
//! The store is dumb and strict: it never resolves a quantity and never
//! repairs a row. [`held_sizing_apply`] applies an ordered batch of
//! transitions in ONE transaction. Retrying an already-applied batch is a
//! no-op; an entry may never overwrite an active entry, skip a generation, or
//! replay a stale one.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub const HELD_SIZING_STATUS_ACTIVE: &str = "active";
pub const HELD_SIZING_STATUS_RELEASED: &str = "released";

/// One stored row (timestamps excluded: they are write metadata, not identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSizingRow {
    pub deployment_id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub state_version: i32,
    pub entry_generation: i64,
    pub status: String,
    pub policy_id: String,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub max_target_qty_micros: Option<i64>,
    pub max_notional_usd: Option<i64>,
    pub resolved_target_qty_micros: i64,
    pub reference_bar_end_ts: i64,
    pub reference_price_micros: i64,
}

impl HeldSizingRow {
    /// Everything that identifies the ENTRY (all but `status`).
    fn same_entry(&self, other: &Self) -> bool {
        Self {
            status: String::new(),
            ..self.clone()
        } == Self {
            status: String::new(),
            ..other.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldSizingTransitionRow {
    Entered(HeldSizingRow),
    /// The entry's row with `status == released`.
    Released(HeldSizingRow),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldSizingApplyReport {
    pub applied: usize,
    pub already_applied: usize,
}

const COLUMNS: &str = "deployment_id, strategy_id, symbol, state_version, entry_generation, \
    status, policy_id, allocation_fraction_bps, initial_allocated_capital_micros, \
    max_target_qty_micros, max_notional_usd, resolved_target_qty_micros, \
    reference_bar_end_ts, reference_price_micros";

fn decode(row: &sqlx::postgres::PgRow) -> Result<HeldSizingRow> {
    Ok(HeldSizingRow {
        deployment_id: row.try_get("deployment_id")?,
        strategy_id: row.try_get("strategy_id")?,
        symbol: row.try_get("symbol")?,
        state_version: row.try_get("state_version")?,
        entry_generation: row.try_get("entry_generation")?,
        status: row.try_get("status")?,
        policy_id: row.try_get("policy_id")?,
        allocation_fraction_bps: row.try_get("allocation_fraction_bps")?,
        initial_allocated_capital_micros: row.try_get("initial_allocated_capital_micros")?,
        max_target_qty_micros: row.try_get("max_target_qty_micros")?,
        max_notional_usd: row.try_get("max_notional_usd")?,
        resolved_target_qty_micros: row.try_get("resolved_target_qty_micros")?,
        reference_bar_end_ts: row.try_get("reference_bar_end_ts")?,
        reference_price_micros: row.try_get("reference_price_micros")?,
    })
}

/// Every row of `(deployment_id, strategy_id)`, ordered by symbol. An empty
/// vector means the read succeeded and found nothing; a DB error is an `Err`,
/// never an empty result.
pub async fn held_sizing_fetch(
    pool: &PgPool,
    deployment_id: &str,
    strategy_id: &str,
) -> Result<Vec<HeldSizingRow>> {
    let rows = sqlx::query(&format!(
        "select {COLUMNS} from sys_strategy_held_sizing_state \
         where deployment_id = $1 and strategy_id = $2 order by symbol"
    ))
    .bind(deployment_id)
    .bind(strategy_id)
    .fetch_all(pool)
    .await
    .context("held_sizing_fetch failed")?;
    rows.iter().map(decode).collect()
}

async fn fetch_for_update(
    tx: &mut Transaction<'_, Postgres>,
    row: &HeldSizingRow,
) -> Result<Option<HeldSizingRow>> {
    let existing = sqlx::query(&format!(
        "select {COLUMNS} from sys_strategy_held_sizing_state \
         where deployment_id = $1 and strategy_id = $2 and symbol = $3 for update"
    ))
    .bind(&row.deployment_id)
    .bind(&row.strategy_id)
    .bind(&row.symbol)
    .fetch_optional(&mut **tx)
    .await
    .context("held sizing row lock failed")?;
    existing.as_ref().map(decode).transpose()
}

async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    r: &HeldSizingRow,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "insert into sys_strategy_held_sizing_state (deployment_id, strategy_id, symbol, \
         state_version, entry_generation, status, policy_id, allocation_fraction_bps, \
         initial_allocated_capital_micros, max_target_qty_micros, max_notional_usd, \
         resolved_target_qty_micros, reference_bar_end_ts, reference_price_micros, \
         recorded_at_utc, released_at_utc) \
         values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,null)",
    )
    .bind(&r.deployment_id)
    .bind(&r.strategy_id)
    .bind(&r.symbol)
    .bind(r.state_version)
    .bind(r.entry_generation)
    .bind(&r.status)
    .bind(&r.policy_id)
    .bind(r.allocation_fraction_bps)
    .bind(r.initial_allocated_capital_micros)
    .bind(r.max_target_qty_micros)
    .bind(r.max_notional_usd)
    .bind(r.resolved_target_qty_micros)
    .bind(r.reference_bar_end_ts)
    .bind(r.reference_price_micros)
    .bind(now)
    .execute(&mut **tx)
    .await
    .context("held sizing insert failed")?;
    Ok(())
}

async fn apply_one(
    tx: &mut Transaction<'_, Postgres>,
    t: &HeldSizingTransitionRow,
    now: DateTime<Utc>,
    report: &mut HeldSizingApplyReport,
) -> Result<()> {
    match t {
        HeldSizingTransitionRow::Entered(new) => {
            if new.status != HELD_SIZING_STATUS_ACTIVE {
                bail!("held sizing entry must carry status active");
            }
            match fetch_for_update(tx, new).await? {
                None => {
                    if new.entry_generation != 1 {
                        bail!(
                            "held sizing entry generation {} has no predecessor",
                            new.entry_generation
                        );
                    }
                    insert(tx, new, now).await?;
                    report.applied += 1;
                }
                Some(existing) if existing.entry_generation == new.entry_generation => {
                    if existing.same_entry(new) {
                        report.already_applied += 1;
                    } else {
                        bail!("held sizing entry conflicts with the stored entry of the same generation");
                    }
                }
                Some(existing) if existing.entry_generation + 1 == new.entry_generation => {
                    if existing.status != HELD_SIZING_STATUS_RELEASED {
                        bail!("held sizing entry would overwrite an active entry");
                    }
                    sqlx::query(
                        "delete from sys_strategy_held_sizing_state \
                         where deployment_id = $1 and strategy_id = $2 and symbol = $3",
                    )
                    .bind(&new.deployment_id)
                    .bind(&new.strategy_id)
                    .bind(&new.symbol)
                    .execute(&mut **tx)
                    .await
                    .context("held sizing re-entry delete failed")?;
                    insert(tx, new, now).await?;
                    report.applied += 1;
                }
                Some(existing) => bail!(
                    "held sizing entry generation {} is stale or skips ahead of stored generation {}",
                    new.entry_generation,
                    existing.entry_generation
                ),
            }
        }
        HeldSizingTransitionRow::Released(rel) => {
            if rel.status != HELD_SIZING_STATUS_RELEASED {
                bail!("held sizing release must carry status released");
            }
            let existing = fetch_for_update(tx, rel)
                .await?
                .ok_or_else(|| anyhow!("held sizing release has no stored entry"))?;
            if existing.entry_generation != rel.entry_generation || !existing.same_entry(rel) {
                bail!("held sizing release does not match the stored entry");
            }
            if existing.status == HELD_SIZING_STATUS_RELEASED {
                report.already_applied += 1;
            } else {
                sqlx::query(
                    "update sys_strategy_held_sizing_state \
                     set status = 'released', released_at_utc = $4 \
                     where deployment_id = $1 and strategy_id = $2 and symbol = $3 \
                       and entry_generation = $5 and status = 'active'",
                )
                .bind(&rel.deployment_id)
                .bind(&rel.strategy_id)
                .bind(&rel.symbol)
                .bind(now)
                .bind(rel.entry_generation)
                .execute(&mut **tx)
                .await
                .context("held sizing release failed")?;
                report.applied += 1;
            }
        }
    }
    Ok(())
}

/// Apply `transitions` in order inside one transaction: all or nothing.
pub async fn held_sizing_apply(
    pool: &PgPool,
    transitions: &[HeldSizingTransitionRow],
    now_utc: DateTime<Utc>,
) -> Result<HeldSizingApplyReport> {
    let mut tx = pool
        .begin()
        .await
        .context("held_sizing_apply begin failed")?;
    let mut report = HeldSizingApplyReport::default();
    for t in transitions {
        apply_one(&mut tx, t, now_utc, &mut report).await?;
    }
    tx.commit()
        .await
        .context("held_sizing_apply commit failed")?;
    Ok(report)
}
