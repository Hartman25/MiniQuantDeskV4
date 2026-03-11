//! B3: Leader Lease — Single Runtime Enforcement
//!
//! Provides DB-backed CAS acquire / refresh / verify / release for the
//! single-row `runtime_leader_lease` table (migration 0018).
//!
//! # [T]-guard compliance
//!
//! No function here calls `Utc::now()` directly.  All functions that need a
//! timestamp accept `now_utc: DateTime<Utc>` from the caller.
//!
//! # Fail-closed semantics
//!
//! - `acquire_lease` returns `HeldByOther` rather than `Acquired` if the
//!   lease is still held by a different process.
//! - `refresh_lease` returns `Err` if the epoch has changed (stolen lease).
//! - `verify_lease` returns `false` on any uncertainty (stolen, expired, or
//!   missing).  DB access failures propagate as `Err`.
//! - `release_lease` is best-effort: silently succeeds if the row is already
//!   gone or held by another holder.

use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The single-row leader lease record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeaderLease {
    pub holder_id: String,
    pub epoch: i64,
    pub lease_expires_at: DateTime<Utc>,
}

/// Outcome of an [`acquire_lease`] call.
#[derive(Debug, Clone)]
pub enum LeaseAcquireOutcome {
    /// This caller now holds the lease.  The inner value is the freshly
    /// written lease record (includes the new epoch).
    Acquired(RuntimeLeaderLease),
    /// The lease is currently held by a different `holder_id`.
    /// The inner value is the current lease record (for operator diagnostics).
    HeldByOther(RuntimeLeaderLease),
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Attempt to acquire the leader lease via a compare-and-swap upsert.
///
/// # Semantics
///
/// | Current DB state                         | Result                        |
/// |------------------------------------------|-------------------------------|
/// | No row (first ever acquire)              | `Acquired(epoch = 1)`         |
/// | Row exists, expired                      | `Acquired(epoch = prev + 1)`  |
/// | Row exists, not expired, **same** holder | `Acquired(epoch = prev + 1)`  |
/// | Row exists, not expired, other holder    | `HeldByOther(current_lease)`  |
///
/// The epoch is always incremented on acquisition to invalidate any in-flight
/// [`refresh_lease`] calls from a previous holder instance.
///
/// # [T]-guard compliance
///
/// `now_utc` must be supplied by the caller.  This function never calls
/// `Utc::now()`.
pub async fn acquire_lease(
    pool: &PgPool,
    holder_id: &str,
    now_utc: DateTime<Utc>,
    ttl_secs: i64,
) -> anyhow::Result<LeaseAcquireOutcome> {
    if ttl_secs <= 0 {
        return Err(anyhow!(
            "acquire_lease: ttl_secs must be > 0, got {ttl_secs}"
        ));
    }

    let new_expiry = now_utc + Duration::seconds(ttl_secs);

    // CAS upsert:
    //
    //   INSERT if no row exists                    → epoch = 1
    //   ON CONFLICT DO UPDATE only when:           → epoch = old + 1
    //     lease is expired (lease_expires_at < $3) OR same holder
    //
    // When the ON CONFLICT WHERE clause is FALSE (unexpired lease, other
    // holder), PostgreSQL skips the DO UPDATE entirely and RETURNING emits
    // zero rows — we detect HeldByOther by the absence of a returned row.
    let row: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        INSERT INTO runtime_leader_lease (id, holder_id, epoch, lease_expires_at, updated_at)
        VALUES (1, $1, 1, $2, $3)
        ON CONFLICT (id) DO UPDATE
          SET holder_id        = excluded.holder_id,
              epoch            = runtime_leader_lease.epoch + 1,
              lease_expires_at = excluded.lease_expires_at,
              updated_at       = excluded.updated_at
        WHERE runtime_leader_lease.lease_expires_at < $3
           OR runtime_leader_lease.holder_id = $1
        RETURNING holder_id, epoch, lease_expires_at
        "#,
    )
    .bind(holder_id)
    .bind(new_expiry)
    .bind(now_utc)
    .fetch_optional(pool)
    .await
    .context("acquire_lease failed")?;

    if let Some((h, epoch, exp)) = row {
        return Ok(LeaseAcquireOutcome::Acquired(RuntimeLeaderLease {
            holder_id: h,
            epoch,
            lease_expires_at: exp,
        }));
    }

    // CAS WHERE was false — another holder owns an unexpired lease.
    let current = fetch_current_lease(pool)
        .await?
        .ok_or_else(|| anyhow!("acquire_lease: conflict detected but row vanished"))?;

    Ok(LeaseAcquireOutcome::HeldByOther(current))
}

/// Refresh (extend) an existing lease.
///
/// Extends `lease_expires_at` by `ttl_secs` from `now_utc`.  The epoch is
/// intentionally **not** changed on refresh — it is only incremented by
/// [`acquire_lease`].
///
/// The UPDATE is conditioned on `holder_id = $1 AND epoch = $2`.  If either
/// check fails (stolen lease, epoch changed after re-acquisition, or missing
/// row), the function returns `Err`.  Callers must treat any error as a fatal
/// lease-loss event and stop dispatch immediately.
///
/// # [T]-guard compliance
///
/// `now_utc` must be supplied by the caller.  This function never calls
/// `Utc::now()`.
pub async fn refresh_lease(
    pool: &PgPool,
    holder_id: &str,
    epoch: i64,
    now_utc: DateTime<Utc>,
    ttl_secs: i64,
) -> anyhow::Result<RuntimeLeaderLease> {
    if ttl_secs <= 0 {
        return Err(anyhow!(
            "refresh_lease: ttl_secs must be > 0, got {ttl_secs}"
        ));
    }

    let new_expiry = now_utc + Duration::seconds(ttl_secs);

    let row: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        UPDATE runtime_leader_lease
          SET lease_expires_at = $4,
              updated_at       = $3
        WHERE id        = 1
          AND holder_id = $1
          AND epoch     = $2
        RETURNING holder_id, epoch, lease_expires_at
        "#,
    )
    .bind(holder_id)
    .bind(epoch)
    .bind(now_utc)
    .bind(new_expiry)
    .fetch_optional(pool)
    .await
    .context("refresh_lease failed")?;

    row.map(|(h, e, exp)| RuntimeLeaderLease {
        holder_id: h,
        epoch: e,
        lease_expires_at: exp,
    })
    .ok_or_else(|| {
        anyhow!(
            "refresh_lease: lease lost \
             (holder={holder_id} epoch={epoch}) — stolen, epoch changed, or row missing"
        )
    })
}

/// Verify that the caller still holds the lease.
///
/// Returns `true` iff the DB row has matching `holder_id`, matching `epoch`,
/// **and** `lease_expires_at > now_utc`.
///
/// Returns `false` (not `Err`) for any "not valid" state: stolen, expired,
/// or row absent.  Returns `Err` only when the DB query itself fails.
///
/// This is read-only — it never mutates the DB.
///
/// # [T]-guard compliance
///
/// `now_utc` must be supplied by the caller.  This function never calls
/// `Utc::now()`.
pub async fn verify_lease(
    pool: &PgPool,
    holder_id: &str,
    epoch: i64,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let current = fetch_current_lease(pool).await?;
    Ok(match current {
        None => false,
        Some(lease) => {
            lease.holder_id == holder_id
                && lease.epoch == epoch
                && lease.lease_expires_at > now_utc
        }
    })
}

/// Release the lease (best-effort).
///
/// Deletes the single row only when both `holder_id` and `epoch` match.
/// Silently succeeds if the row is absent, or if another holder owns it.
///
/// Most callers should invoke this on graceful shutdown.  Lease expiry is the
/// primary enforcement mechanism; `release_lease` is a courtesy that lets the
/// next holder acquire immediately rather than waiting for TTL.
pub async fn release_lease(pool: &PgPool, holder_id: &str, epoch: i64) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        DELETE FROM runtime_leader_lease
        WHERE id        = 1
          AND holder_id = $1
          AND epoch     = $2
        "#,
    )
    .bind(holder_id)
    .bind(epoch)
    .execute(pool)
    .await
    .context("release_lease failed")?;
    Ok(())
}

/// Fetch the current lease row (read-only).
///
/// Returns `None` if the table is empty — no lease has been acquired yet.
pub async fn fetch_current_lease(pool: &PgPool) -> anyhow::Result<Option<RuntimeLeaderLease>> {
    let row: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT holder_id, epoch, lease_expires_at
        FROM runtime_leader_lease
        WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("fetch_current_lease failed")?;

    Ok(row.map(|(holder_id, epoch, lease_expires_at)| RuntimeLeaderLease {
        holder_id,
        epoch,
        lease_expires_at,
    }))
}
