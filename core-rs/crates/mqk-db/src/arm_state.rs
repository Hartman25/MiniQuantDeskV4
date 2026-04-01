use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Arm state persistence — Patch L7
// ---------------------------------------------------------------------------

/// Persist the current arm state to `sys_arm_state` (upsert singleton row).
///
/// `state` must be `"ARMED"` or `"DISARMED"`.
/// `reason` is the `DisarmReason` variant name when `state == "DISARMED"`;
/// pass `None` (or `Some("BootDefault")`) when arming.
pub async fn persist_arm_state(pool: &PgPool, state: &str, reason: Option<&str>) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_arm_state (sentinel_id, state, reason, updated_at_utc)
        values (1, $1, $2, now()) -- allow: ops-metadata
        on conflict (sentinel_id) do update
            set state          = excluded.state,
                reason         = excluded.reason,
                updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(state)
    .bind(reason)
    .execute(pool)
    .await
    .context("persist_arm_state failed")?;
    Ok(())
}

/// Load the last persisted arm state.
///
/// Returns `None` if no state has ever been persisted (fresh system —
/// should treat this as `DISARMED / BootDefault`).
///
/// Returns `Some((state, reason))` where `state` is `"ARMED"` or `"DISARMED"`
/// and `reason` is the `DisarmReason` variant name (or `None` when armed).
pub async fn load_arm_state(pool: &PgPool) -> Result<Option<(String, Option<String>)>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        select state, reason
        from sys_arm_state
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("load_arm_state failed")?;
    Ok(row)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmState {
    Armed,
    Disarmed,
}

impl ArmState {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Armed => "ARMED",
            Self::Disarmed => "DISARMED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisarmReason {
    OperatorDisarm,
    OperatorHalt,
    ReconcileDrift,
    DeadmanExpired,
    DeadmanSupervisorFailure,
    DeadmanHeartbeatPersistFailed,
}

impl DisarmReason {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::OperatorDisarm => "OperatorDisarm",
            Self::OperatorHalt => "OperatorHalt",
            Self::ReconcileDrift => "ReconcileDrift",
            Self::DeadmanExpired => "DeadmanExpired",
            Self::DeadmanSupervisorFailure => "DeadmanSupervisorFailure",
            Self::DeadmanHeartbeatPersistFailed => "DeadmanHeartbeatPersistFailed",
        }
    }
}

pub async fn persist_arm_state_canonical(
    pool: &PgPool,
    state: ArmState,
    reason: Option<DisarmReason>,
) -> Result<()> {
    let reason_str = match state {
        ArmState::Armed => None,
        ArmState::Disarmed => reason.map(DisarmReason::as_db_str),
    };
    persist_arm_state(pool, state.as_db_str(), reason_str).await
}

#[derive(Debug, Clone)]
pub struct RiskBlockState {
    pub blocked: bool,
    pub reason: Option<String>,
    pub updated_at_utc: DateTime<Utc>,
}

/// Persist the current risk-block posture to `sys_risk_block_state` (singleton row).
pub async fn persist_risk_block_state(
    pool: &PgPool,
    blocked: bool,
    reason: Option<&str>,
    updated_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_risk_block_state (sentinel_id, blocked, reason, updated_at_utc)
        values (1, $1, $2, $3)
        on conflict (sentinel_id) do update
            set blocked        = excluded.blocked,
                reason         = excluded.reason,
                updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(blocked)
    .bind(reason)
    .bind(updated_at)
    .execute(pool)
    .await
    .context("persist_risk_block_state failed")?;
    Ok(())
}

/// Load the last persisted risk-block posture.
pub async fn load_risk_block_state(pool: &PgPool) -> Result<Option<RiskBlockState>> {
    let row = sqlx::query(
        r#"
        select blocked, reason, updated_at_utc
        from sys_risk_block_state
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("load_risk_block_state failed")?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(RiskBlockState {
        blocked: row.try_get("blocked")?,
        reason: row.try_get("reason")?,
        updated_at_utc: row.try_get("updated_at_utc")?,
    }))
}

// ---------------------------------------------------------------------------
// RD-01: Durable risk denial event history (sys_risk_denial_events)
// ---------------------------------------------------------------------------

/// One row from `sys_risk_denial_events`.
#[derive(Debug, Clone)]
pub struct RiskDenialEventRow {
    /// Deterministic display ID: `"{denied_at_utc_micros}:{rule_code}"`.
    /// Unique for all practical purposes within a deployment.
    pub id: String,
    /// UTC timestamp when the denial was captured.
    pub denied_at_utc: DateTime<Utc>,
    /// Machine-readable rule code, e.g. `"POSITION_LIMIT_EXCEEDED"`.
    pub rule: String,
    /// Human-readable one-line summary from `RiskReason::as_summary()`.
    pub message: String,
    /// Symbol from the denied order.
    pub symbol: Option<String>,
    /// Requested order quantity, if populated by the risk rule.
    pub requested_qty: Option<i64>,
    /// Configured limit that was breached, if populated by the risk rule.
    pub limit_qty: Option<i64>,
    /// Always `"critical"` for risk gate denials.
    pub severity: String,
}

/// Persist a single risk denial event to `sys_risk_denial_events`.
///
/// Uses `ON CONFLICT (id) DO NOTHING` — idempotent.  Repeated inserts for
/// the same deterministic `id` are silent no-ops, so best-effort writes from
/// the orchestrator can never double-count a denial.
pub async fn persist_risk_denial_event(pool: &PgPool, row: &RiskDenialEventRow) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_risk_denial_events
            (id, denied_at_utc, rule, message, symbol, requested_qty, limit_qty, severity)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (id) do nothing
        "#,
    )
    .bind(&row.id)
    .bind(row.denied_at_utc)
    .bind(&row.rule)
    .bind(&row.message)
    .bind(&row.symbol)
    .bind(row.requested_qty)
    .bind(row.limit_qty)
    .bind(&row.severity)
    .execute(pool)
    .await
    .context("persist_risk_denial_event failed")?;
    Ok(())
}

/// Load the most recent `limit` risk denial events ordered newest-first.
///
/// Used by `GET /api/v1/risk/denials` to surface restart-safe denial history.
/// Returns an empty `Vec` if no denials have been recorded yet.
pub async fn load_recent_risk_denial_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<RiskDenialEventRow>> {
    let rows = sqlx::query(
        r#"
        select id, denied_at_utc, rule, message, symbol, requested_qty, limit_qty, severity
        from sys_risk_denial_events
        order by denied_at_utc desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("load_recent_risk_denial_events failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(RiskDenialEventRow {
            id: r.try_get("id")?,
            denied_at_utc: r.try_get("denied_at_utc")?,
            rule: r.try_get("rule")?,
            message: r.try_get("message")?,
            symbol: r.try_get("symbol")?,
            requested_qty: r.try_get("requested_qty")?,
            limit_qty: r.try_get("limit_qty")?,
            severity: r.try_get("severity")?,
        });
    }
    Ok(out)
}


// ---------------------------------------------------------------------------
// AUTON-PAPER-02: Durable autonomous-session supervisor history
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AutonomousSessionEventRow {
    pub id: String,
    pub ts_utc: DateTime<Utc>,
    pub event_type: String,
    pub resume_source: Option<String>,
    pub detail: String,
    pub run_id: Option<Uuid>,
    pub source: String,
}

pub async fn persist_autonomous_session_event(
    pool: &PgPool,
    row: &AutonomousSessionEventRow,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_autonomous_session_events
            (id, ts_utc, event_type, resume_source, detail, run_id, source)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (id) do nothing
        "#,
    )
    .bind(&row.id)
    .bind(row.ts_utc)
    .bind(&row.event_type)
    .bind(&row.resume_source)
    .bind(&row.detail)
    .bind(row.run_id)
    .bind(&row.source)
    .execute(pool)
    .await
    .context("persist_autonomous_session_event failed")?;
    Ok(())
}

pub async fn load_recent_autonomous_session_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AutonomousSessionEventRow>> {
    let rows = sqlx::query(
        r#"
        select id, ts_utc, event_type, resume_source, detail, run_id, source
        from sys_autonomous_session_events
        order by ts_utc desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("load_recent_autonomous_session_events failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(AutonomousSessionEventRow {
            id: r.try_get("id")?,
            ts_utc: r.try_get("ts_utc")?,
            event_type: r.try_get("event_type")?,
            resume_source: r.try_get("resume_source")?,
            detail: r.try_get("detail")?,
            run_id: r.try_get("run_id")?,
            source: r.try_get("source")?,
        });
    }
    Ok(out)
}
