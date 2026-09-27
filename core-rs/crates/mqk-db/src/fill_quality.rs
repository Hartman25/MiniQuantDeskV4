use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// TV-EXEC-01: Fill-quality telemetry
// ---------------------------------------------------------------------------

/// Row to insert into `fill_quality_telemetry`.
///
/// `telemetry_id` must be a deterministic UUIDv5 derived by the caller so that
/// repeated best-effort writes from the orchestrator are idempotent.
#[derive(Debug, Clone)]
pub struct NewFillQualityTelemetry {
    pub telemetry_id: Uuid,
    pub run_id: Uuid,
    pub internal_order_id: String,
    pub broker_order_id: Option<String>,
    pub broker_fill_id: Option<String>,
    pub broker_message_id: String,
    pub symbol: String,
    /// `"buy"` or `"sell"`
    pub side: String,
    /// D6/A5/D7: exactly one of (`ordered_qty`, `ordered_qty_micros`) is a
    /// meaningful value per row, or both are `None` when the order quantity
    /// was never resolvable (best-effort outbox lookup miss) -- see
    /// `fill_quality_telemetry_ordered_qty_encoding_check` (migration 0079).
    /// Ordered qty from outbox order_json (`qty` field). Whole-unit only.
    pub ordered_qty: Option<i64>,
    /// Raw `QtyMicros` ordered quantity (whole or fractional). `None` when
    /// `ordered_qty` carries the value, or when never resolvable.
    pub ordered_qty_micros: Option<i64>,
    /// D6/A5/D7: exactly one of (`fill_qty`, `fill_qty_micros`) is `Some` --
    /// see `fill_quality_telemetry_fill_qty_encoding_check` (migration
    /// 0079). Never both `None` (a fill event always has a positive
    /// quantity).
    /// Delta fill qty for this event. Whole-unit only; `None` for a
    /// genuinely fractional fill.
    pub fill_qty: Option<i64>,
    /// Raw `QtyMicros` delta fill quantity (whole or fractional). `None`
    /// when `fill_qty` carries the value.
    pub fill_qty_micros: Option<i64>,
    /// `"qty_micros_v1"` for a row written by this encoding; `None` for a
    /// historical row written before this schema existed (whole-unit
    /// columns are authoritative for those).
    pub quantity_schema_version: Option<String>,
    /// Actual executed price in micros.
    pub fill_price_micros: i64,
    /// Limit price in micros from outbox order_json. `None` for market orders.
    pub reference_price_micros: Option<i64>,
    /// Signed slippage in basis points. `None` when reference_price_micros is absent.
    pub slippage_bps: Option<i64>,
    /// `outbox.sent_at_utc` — `None` if outbox row is absent or not yet sent.
    pub submit_ts_utc: Option<DateTime<Utc>>,
    /// `inbox.received_at_utc` for this fill event.
    pub fill_received_at_utc: DateTime<Utc>,
    /// Derived latency in milliseconds. `None` when submit_ts_utc is absent.
    pub submit_to_fill_ms: Option<i64>,
    /// `"partial_fill"` or `"final_fill"`
    pub fill_kind: String,
    /// Always `"oms_inbox:{broker_message_id}"`.
    pub provenance_ref: String,
    pub created_at_utc: DateTime<Utc>,
}

/// A fill-quality row returned by the read path.
#[derive(Debug, Clone)]
pub struct FillQualityRow {
    pub telemetry_id: Uuid,
    pub run_id: Uuid,
    pub internal_order_id: String,
    pub broker_order_id: Option<String>,
    pub broker_fill_id: Option<String>,
    pub broker_message_id: String,
    pub symbol: String,
    pub side: String,
    pub ordered_qty: Option<i64>,
    pub ordered_qty_micros: Option<i64>,
    pub fill_qty: Option<i64>,
    pub fill_qty_micros: Option<i64>,
    pub quantity_schema_version: Option<String>,
    pub fill_price_micros: i64,
    pub reference_price_micros: Option<i64>,
    pub slippage_bps: Option<i64>,
    pub submit_ts_utc: Option<DateTime<Utc>>,
    pub fill_received_at_utc: DateTime<Utc>,
    pub submit_to_fill_ms: Option<i64>,
    pub fill_kind: String,
    pub provenance_ref: String,
    pub created_at_utc: DateTime<Utc>,
}

/// Persist a single fill-quality telemetry row.
///
/// Idempotent via `ON CONFLICT (telemetry_id) DO NOTHING` — repeated
/// best-effort writes from the orchestrator can never double-count a fill.
pub async fn insert_fill_quality_telemetry(
    pool: &PgPool,
    row: &NewFillQualityTelemetry,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into fill_quality_telemetry (
            telemetry_id, run_id, internal_order_id, broker_order_id, broker_fill_id,
            broker_message_id, symbol, side, ordered_qty, fill_qty,
            fill_price_micros, reference_price_micros, slippage_bps,
            submit_ts_utc, fill_received_at_utc, submit_to_fill_ms,
            fill_kind, provenance_ref, created_at_utc,
            quantity_schema_version, ordered_qty_micros, fill_qty_micros
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
        on conflict (telemetry_id) do nothing
        "#,
    )
    .bind(row.telemetry_id)
    .bind(row.run_id)
    .bind(&row.internal_order_id)
    .bind(&row.broker_order_id)
    .bind(&row.broker_fill_id)
    .bind(&row.broker_message_id)
    .bind(&row.symbol)
    .bind(&row.side)
    .bind(row.ordered_qty)
    .bind(row.fill_qty)
    .bind(row.fill_price_micros)
    .bind(row.reference_price_micros)
    .bind(row.slippage_bps)
    .bind(row.submit_ts_utc)
    .bind(row.fill_received_at_utc)
    .bind(row.submit_to_fill_ms)
    .bind(&row.fill_kind)
    .bind(&row.provenance_ref)
    .bind(row.created_at_utc)
    .bind(&row.quantity_schema_version)
    .bind(row.ordered_qty_micros)
    .bind(row.fill_qty_micros)
    .execute(pool)
    .await
    .context("insert_fill_quality_telemetry failed")?;
    Ok(())
}

/// Load fill-quality rows for a specific order in a run, oldest-fill first.
///
/// Returns at most 50 rows in ascending `fill_received_at_utc` order so callers
/// see the chronological fill sequence for a per-order timeline.
pub async fn fetch_fill_quality_telemetry_for_order(
    pool: &PgPool,
    run_id: Uuid,
    internal_order_id: &str,
) -> Result<Vec<FillQualityRow>> {
    let rows = sqlx::query(
        r#"
        select telemetry_id, run_id, internal_order_id, broker_order_id, broker_fill_id,
               broker_message_id, symbol, side, ordered_qty, fill_qty,
               fill_price_micros, reference_price_micros, slippage_bps,
               submit_ts_utc, fill_received_at_utc, submit_to_fill_ms,
               fill_kind, provenance_ref, created_at_utc,
               quantity_schema_version, ordered_qty_micros, fill_qty_micros
        from fill_quality_telemetry
        where run_id = $1
          and internal_order_id = $2
        order by fill_received_at_utc asc
        limit 50
        "#,
    )
    .bind(run_id)
    .bind(internal_order_id)
    .fetch_all(pool)
    .await
    .context("fetch_fill_quality_telemetry_for_order failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(FillQualityRow {
            telemetry_id: r.try_get("telemetry_id")?,
            run_id: r.try_get("run_id")?,
            internal_order_id: r.try_get("internal_order_id")?,
            broker_order_id: r.try_get("broker_order_id")?,
            broker_fill_id: r.try_get("broker_fill_id")?,
            broker_message_id: r.try_get("broker_message_id")?,
            symbol: r.try_get("symbol")?,
            side: r.try_get("side")?,
            ordered_qty: r.try_get("ordered_qty")?,
            fill_qty: r.try_get("fill_qty")?,
            fill_price_micros: r.try_get("fill_price_micros")?,
            reference_price_micros: r.try_get("reference_price_micros")?,
            slippage_bps: r.try_get("slippage_bps")?,
            submit_ts_utc: r.try_get("submit_ts_utc")?,
            fill_received_at_utc: r.try_get("fill_received_at_utc")?,
            submit_to_fill_ms: r.try_get("submit_to_fill_ms")?,
            fill_kind: r.try_get("fill_kind")?,
            provenance_ref: r.try_get("provenance_ref")?,
            created_at_utc: r.try_get("created_at_utc")?,
            quantity_schema_version: r.try_get("quantity_schema_version")?,
            ordered_qty_micros: r.try_get("ordered_qty_micros")?,
            fill_qty_micros: r.try_get("fill_qty_micros")?,
        });
    }
    Ok(out)
}

/// Load fill-quality rows for a specific order across ALL runs, oldest-fill first.
///
/// `internal_order_id` (a deterministic decision_id) is durable order identity
/// independent of `run_id`. A fill is recorded under the `run_id` that was
/// active at the moment the fill was applied; that run may no longer be the
/// currently active run (e.g. after an autonomous session recovery-restart
/// assigns a new `run_id` while in-memory OMS order state carries over
/// unchanged). This fallback lets per-order surfaces find durably-recorded
/// fill evidence regardless of which run recorded it.
///
/// Returns at most 50 rows in ascending `fill_received_at_utc` order.
pub async fn fetch_fill_quality_telemetry_for_order_any_run(
    pool: &PgPool,
    internal_order_id: &str,
) -> Result<Vec<FillQualityRow>> {
    let rows = sqlx::query(
        r#"
        select telemetry_id, run_id, internal_order_id, broker_order_id, broker_fill_id,
               broker_message_id, symbol, side, ordered_qty, fill_qty,
               fill_price_micros, reference_price_micros, slippage_bps,
               submit_ts_utc, fill_received_at_utc, submit_to_fill_ms,
               fill_kind, provenance_ref, created_at_utc,
               quantity_schema_version, ordered_qty_micros, fill_qty_micros
        from fill_quality_telemetry
        where internal_order_id = $1
        order by fill_received_at_utc asc
        limit 50
        "#,
    )
    .bind(internal_order_id)
    .fetch_all(pool)
    .await
    .context("fetch_fill_quality_telemetry_for_order_any_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(FillQualityRow {
            telemetry_id: r.try_get("telemetry_id")?,
            run_id: r.try_get("run_id")?,
            internal_order_id: r.try_get("internal_order_id")?,
            broker_order_id: r.try_get("broker_order_id")?,
            broker_fill_id: r.try_get("broker_fill_id")?,
            broker_message_id: r.try_get("broker_message_id")?,
            symbol: r.try_get("symbol")?,
            side: r.try_get("side")?,
            ordered_qty: r.try_get("ordered_qty")?,
            fill_qty: r.try_get("fill_qty")?,
            fill_price_micros: r.try_get("fill_price_micros")?,
            reference_price_micros: r.try_get("reference_price_micros")?,
            slippage_bps: r.try_get("slippage_bps")?,
            submit_ts_utc: r.try_get("submit_ts_utc")?,
            fill_received_at_utc: r.try_get("fill_received_at_utc")?,
            submit_to_fill_ms: r.try_get("submit_to_fill_ms")?,
            fill_kind: r.try_get("fill_kind")?,
            provenance_ref: r.try_get("provenance_ref")?,
            created_at_utc: r.try_get("created_at_utc")?,
            quantity_schema_version: r.try_get("quantity_schema_version")?,
            ordered_qty_micros: r.try_get("ordered_qty_micros")?,
            fill_qty_micros: r.try_get("fill_qty_micros")?,
        });
    }
    Ok(out)
}

/// Load the most recent `limit` fill-quality rows for a run, newest-fill first.
pub async fn fetch_fill_quality_telemetry_recent(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<FillQualityRow>> {
    let rows = sqlx::query(
        r#"
        select telemetry_id, run_id, internal_order_id, broker_order_id, broker_fill_id,
               broker_message_id, symbol, side, ordered_qty, fill_qty,
               fill_price_micros, reference_price_micros, slippage_bps,
               submit_ts_utc, fill_received_at_utc, submit_to_fill_ms,
               fill_kind, provenance_ref, created_at_utc,
               quantity_schema_version, ordered_qty_micros, fill_qty_micros
        from fill_quality_telemetry
        where run_id = $1
        order by fill_received_at_utc desc
        limit $2
        "#,
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_fill_quality_telemetry_recent failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(FillQualityRow {
            telemetry_id: r.try_get("telemetry_id")?,
            run_id: r.try_get("run_id")?,
            internal_order_id: r.try_get("internal_order_id")?,
            broker_order_id: r.try_get("broker_order_id")?,
            broker_fill_id: r.try_get("broker_fill_id")?,
            broker_message_id: r.try_get("broker_message_id")?,
            symbol: r.try_get("symbol")?,
            side: r.try_get("side")?,
            ordered_qty: r.try_get("ordered_qty")?,
            fill_qty: r.try_get("fill_qty")?,
            fill_price_micros: r.try_get("fill_price_micros")?,
            reference_price_micros: r.try_get("reference_price_micros")?,
            slippage_bps: r.try_get("slippage_bps")?,
            submit_ts_utc: r.try_get("submit_ts_utc")?,
            fill_received_at_utc: r.try_get("fill_received_at_utc")?,
            submit_to_fill_ms: r.try_get("submit_to_fill_ms")?,
            fill_kind: r.try_get("fill_kind")?,
            provenance_ref: r.try_get("provenance_ref")?,
            created_at_utc: r.try_get("created_at_utc")?,
            quantity_schema_version: r.try_get("quantity_schema_version")?,
            ordered_qty_micros: r.try_get("ordered_qty_micros")?,
            fill_qty_micros: r.try_get("fill_qty_micros")?,
        });
    }
    Ok(out)
}
