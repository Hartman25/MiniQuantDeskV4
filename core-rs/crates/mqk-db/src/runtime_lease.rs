//! DB-backed runtime leader lease.
//!
//! Provides atomic acquire / refresh / verify / release for the single-row
//! `runtime_leader_lease` table.
//!
//! Fail-closed contract:
//! - acquisition only succeeds when the row is absent or expired
//! - refresh only succeeds for the current holder + current epoch + unexpired row
//! - verify returns false on any ambiguity (missing, expired, or mismatched)
//! - release deletes only the exact holder/epoch pair

use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// RUNTIME-LEASE-RUN-IDENTITY-AUTHORITY-01: canonical TTL constants
// ---------------------------------------------------------------------------

/// Canonical runtime leadership lease TTL (seconds). Single source of truth
/// for `mqk-runtime`'s `ExecutionOrchestrator` -- do not redefine this value
/// locally in another crate; import this constant instead. See
/// `DEADMAN_TTL_SECS` for why the deadman TTL is deliberately a different,
/// larger value, and [`acquire_or_refresh_lease_for_running_run`]'s doc
/// comment for how the two are reconciled at the moment leadership actually
/// transfers.
pub const RUNTIME_LEASE_TTL_SECS: i64 = 90;

/// Canonical deadman heartbeat TTL (seconds). Single source of truth for
/// `mqk-daemon`'s execution-loop supervisor -- do not redefine this value
/// locally in another crate; import this constant instead.
///
/// Deliberately larger than [`RUNTIME_LEASE_TTL_SECS`]: it must exceed the
/// longest a single orchestrator tick can legitimately block (observed up to
/// ~33s on a slow broker REST call) plus margin, or a merely-slow (not dead)
/// runtime would be falsely halted on its next heartbeat check.
pub const DEADMAN_TTL_SECS: i64 = 120;

/// The single runtime leader lease row.
///
/// `run_id` is `None` only for a legacy row written before
/// RUNTIME-LEASE-RUN-IDENTITY-AUTHORITY-01 (migration 0068) added the
/// column -- every lease acquired by current code always sets it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeLeaderLease {
    pub run_id: Option<Uuid>,
    pub holder_id: String,
    pub epoch: i64,
    pub lease_expires_at: DateTime<Utc>,
}

impl RuntimeLeaderLease {
    pub fn is_expired_at(&self, now_utc: DateTime<Utc>) -> bool {
        self.lease_expires_at <= now_utc
    }
}

/// Outcome of [`acquire_lease`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAcquireOutcome {
    Acquired(RuntimeLeaderLease),
    HeldByOther(RuntimeLeaderLease),
}

/// Acquire leadership when no valid lease exists.
///
/// Atomic DB semantics:
/// - insert when the table is empty
/// - replace the row and increment `epoch` only when the stored lease is expired
/// - otherwise return the currently active lease
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

    let acquired: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        INSERT INTO runtime_leader_lease (id, holder_id, epoch, lease_expires_at, updated_at)
        VALUES (1, $1, 1, $2, $3)
        ON CONFLICT (id) DO UPDATE
          SET holder_id        = excluded.holder_id,
              epoch            = runtime_leader_lease.epoch + 1,
              lease_expires_at = excluded.lease_expires_at,
              updated_at       = excluded.updated_at
        WHERE runtime_leader_lease.lease_expires_at <= $3
        RETURNING holder_id, epoch, lease_expires_at
        "#,
    )
    .bind(holder_id)
    .bind(new_expiry)
    .bind(now_utc)
    .fetch_optional(pool)
    .await
    .context("acquire_lease failed")?;

    if let Some((holder_id, epoch, lease_expires_at)) = acquired {
        return Ok(LeaseAcquireOutcome::Acquired(RuntimeLeaderLease {
            run_id: None,
            holder_id,
            epoch,
            lease_expires_at,
        }));
    }

    let current = fetch_current_lease(pool).await?.ok_or_else(|| {
        anyhow!("acquire_lease: active conflict detected but lease row is missing")
    })?;

    Ok(LeaseAcquireOutcome::HeldByOther(current))
}

/// Renew the current holder's lease without changing the epoch.
///
/// Refresh is compare-and-swap on `(holder_id, epoch)` and also requires the
/// row to still be unexpired at `now_utc`. An expired leader cannot revive its
/// own lease by calling refresh after timeout.
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

    let refreshed: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        UPDATE runtime_leader_lease
           SET lease_expires_at = $4,
               updated_at       = $3
         WHERE id               = 1
           AND holder_id        = $1
           AND epoch            = $2
           AND lease_expires_at > $3
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

    refreshed
        .map(|(holder_id, epoch, lease_expires_at)| RuntimeLeaderLease {
            run_id: None,
            holder_id,
            epoch,
            lease_expires_at,
        })
        .ok_or_else(|| {
            anyhow!(
                "refresh_lease: lease lost (holder={holder_id} epoch={epoch}) \
                 — holder mismatch, epoch mismatch, row missing, or lease expired"
            )
        })
}

/// Verify that `holder_id` and `epoch` still own an unexpired lease.
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
            lease.holder_id == holder_id && lease.epoch == epoch && !lease.is_expired_at(now_utc)
        }
    })
}

/// Release leadership for the exact holder/epoch pair.
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

/// Verify that `run_id`/`holder_id`/`epoch` still own an unexpired lease.
/// Unlike [`verify_lease`], this also requires the lease to be bound to the
/// exact `run_id` supplied -- a lease belonging to a different run (or a
/// legacy row with no run binding at all) never validates another run's
/// authority, regardless of holder/epoch/expiry.
pub async fn verify_lease_for_run(
    pool: &PgPool,
    run_id: Uuid,
    holder_id: &str,
    epoch: i64,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let current = fetch_current_lease(pool).await?;
    Ok(match current {
        None => false,
        Some(lease) => {
            lease.run_id == Some(run_id)
                && lease.holder_id == holder_id
                && lease.epoch == epoch
                && !lease.is_expired_at(now_utc)
        }
    })
}

/// Release leadership for the exact `run_id`/`holder_id`/`epoch` triple.
/// This is the canonical production release path (see
/// `ExecutionOrchestrator::release_runtime_leadership`) -- unlike
/// [`release_lease`], the delete is fenced to the caller's own bound run, so
/// a caller can never delete a lease that (through some other bug) turns out
/// to belong to a different run's holder/epoch pair.
pub async fn release_lease_for_run(
    pool: &PgPool,
    run_id: Uuid,
    holder_id: &str,
    epoch: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        DELETE FROM runtime_leader_lease
         WHERE id        = 1
           AND run_id     = $1
           AND holder_id  = $2
           AND epoch      = $3
        "#,
    )
    .bind(run_id)
    .bind(holder_id)
    .bind(epoch)
    .execute(pool)
    .await
    .context("release_lease_for_run failed")?;
    Ok(())
}

/// Read the current lease row, if present.
pub async fn fetch_current_lease(pool: &PgPool) -> anyhow::Result<Option<RuntimeLeaderLease>> {
    let row: Option<(Option<Uuid>, String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT run_id, holder_id, epoch, lease_expires_at
          FROM runtime_leader_lease
         WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("fetch_current_lease failed")?;

    Ok(row.map(|(run_id, holder_id, epoch, lease_expires_at)| RuntimeLeaderLease {
        run_id,
        holder_id,
        epoch,
        lease_expires_at,
    }))
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-STALE-CLAIM-RECOVERY-03 — run-aware lease authority
// ---------------------------------------------------------------------------

/// Outcome of [`acquire_or_refresh_lease_for_running_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLeaseAuthorityOutcome {
    /// A fresh lease was acquired (no prior epoch held by this caller).
    Acquired(RuntimeLeaderLease),
    /// The caller's existing `(holder_id, epoch)` lease was renewed.
    Refreshed(RuntimeLeaderLease),
    /// The run is not durably `RUNNING` — the caller must not acquire,
    /// refresh, claim, or dispatch, and must NOT treat this as ordinary
    /// lease loss (in particular, must not re-halt a `STOPPED` run).
    RunNotRunning { actual_status: String },
    /// Acquisition was refused: another holder's lease is active and
    /// unexpired.
    HeldByOther(RuntimeLeaderLease),
    /// A refresh was attempted but the CAS failed: holder/epoch mismatch or
    /// the lease already expired. The run is still `RUNNING` — this is
    /// genuine lease loss/contest, distinct from `RunNotRunning`.
    Lost,
}

/// Run-aware, transactionally-fenced lease acquire/refresh.
///
/// # Why this exists (root cause of PAPER-SOAK-STALE-CLAIM-RECOVERY-03)
///
/// [`acquire_lease`] and [`refresh_lease`] validate holder/epoch/expiry
/// against the `runtime_leader_lease` row alone — they have no notion of
/// which `run_id` the lease is being exercised for, or whether that run is
/// still durably `RUNNING`. That let a runtime which observed `RUNNING` at
/// the top of a tick keep successfully refreshing its lease (and therefore
/// keep claiming/dispatching outbox rows) even after an operator halted the
/// run and `clear_halted_run_and_reset_stale_claims` reset its stranded
/// `CLAIMED` row back to `PENDING` — the exact stale-runtime-dispatch race
/// this patch closes.
///
/// # Serialization boundary
///
/// This function locks the target run's row (`SELECT ... FOR UPDATE`)
/// *before* deciding lease authority, and holds that lock for the entire
/// decision + mutation. [`crate::clear_halted_run_and_reset_stale_claims`]
/// locks the exact same row the exact same way before it decides recovery
/// eligibility. Postgres serializes any two transactions that lock the same
/// row: whichever of {this function, a concurrent recovery attempt} reaches
/// its `FOR UPDATE`/`UPDATE` first runs to completion (commit or rollback)
/// before the other's lock acquisition proceeds. This is what makes
/// "observed RUNNING, then raced against a concurrent halt-clear" resolve
/// deterministically instead of via timing — including the case where no
/// `runtime_leader_lease` row exists yet at all (the row being locked is
/// `runs`, not `runtime_leader_lease`, so the lease row's absence does not
/// weaken the fence).
///
/// # `current_epoch`
///
/// `None` requests acquisition (mirrors [`acquire_lease`]); `Some(epoch)`
/// requests a refresh of that exact epoch (mirrors [`refresh_lease`]) — same
/// caller-side branching `ExecutionOrchestrator::refresh_or_acquire_runtime_leadership`
/// already used, now routed through this single fenced entrypoint instead of
/// the two unfenced primitives. A refresh additionally requires the lease's
/// bound `run_id` to equal `run_id` — a CAS that no longer matches (holder,
/// epoch, or run_id mismatch) is reported as ordinary lease loss (`Lost`),
/// exactly like any other CAS failure.
///
/// # Cross-run reconciliation (RUNTIME-LEASE-RUN-IDENTITY-AUTHORITY-01)
///
/// `runtime_leader_lease` is a single global row (migration 0018); before
/// migration 0068 it carried no notion of which run it was acquired for. A
/// rejected earlier attempt at deadman reconciliation
/// (DEADMAN-LEASE-TTL-RECONCILE-01) judged whether an existing, raw-expired
/// lease could be stolen by reading `last_heartbeat_utc` for the run making
/// the acquisition attempt — but an orphaned lease from a different,
/// already-terminated run can survive on disk (`stop_run_if_evidence_clean`
/// does not delete it), so that heartbeat was never evidence about the
/// existing holder at all.
///
/// With `run_id` now durably bound to the lease (migration 0068), the acquire
/// path distinguishes exactly three cases once an existing lease is found to
/// be raw-expired:
///
/// - No existing row: nothing to reconcile against; proceed straight to
///   acquisition.
/// - Legacy row (`run_id IS NULL`, written before migration 0068, or by one
///   of this crate's legacy non-run-aware `acquire_lease`/`refresh_lease`
///   primitives): the owning run is unknowable, so — unlike the same-run
///   case below — no run's heartbeat can ever corroborate or refute its
///   liveness (RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01). Raw expiry
///   alone is therefore NOT sufficient: the row's own `updated_at` (its only
///   available liveness signal) must also be older than `deadman_ttl_secs`
///   before it is treated as orphaned — otherwise a legacy lease could be
///   raw-expired (past the 90s `RUNTIME_LEASE_TTL_SECS`) while whatever holds
///   it is still deadman-healthy (within the larger 120s `DEADMAN_TTL_SECS`).
///   Migration 0069 additionally deletes any such row outright once the
///   whole system is provably quiescent, so this branch is defense in depth
///   for the case one is ever reintroduced.
/// - Same run (`existing.run_id == run_id`): `last_heartbeat_utc` fetched
///   above for the locked target run genuinely belongs to this lease's own
///   owner, so it is valid deadman evidence — the 90s-lease/120s-deadman
///   reconciliation applies (a lease expired only by its own clock is not
///   stealable until deadman independently agrees the owner is gone).
/// - Different run (`existing.run_id` is `Some` and not equal to `run_id`):
///   the target run's heartbeat is never consulted. Disposition comes from
///   the other run's own durable `runs.status` instead. If that run is still
///   `RUNNING`, refuse (fail closed on ambiguous cross-run authority —
///   structurally should not happen given
///   `create_or_reuse_run_for_start`'s single-active-run invariant, but never
///   assumed). Otherwise the lease is orphaned and safe to reclaim
///   immediately: that run's own orchestrator can no longer legitimately
///   refresh it either (its calls already hit `RunNotRunning` before ever
///   reaching the lease), so no deadman wait is needed.
///
/// An unexpired lease is never stealable regardless of any of the above, and
/// the final `INSERT ... ON CONFLICT ... WHERE lease_expires_at <= $now`
/// remains the true atomic guarantee; the reconciliation above only decides
/// whether this call attempts that write and what outcome to report when it
/// deliberately does not.
pub async fn acquire_or_refresh_lease_for_running_run(
    pool: &PgPool,
    run_id: Uuid,
    holder_id: &str,
    current_epoch: Option<i64>,
    now_utc: DateTime<Utc>,
    ttl_secs: i64,
    deadman_ttl_secs: i64,
) -> anyhow::Result<RunLeaseAuthorityOutcome> {
    if ttl_secs <= 0 {
        return Err(anyhow!(
            "acquire_or_refresh_lease_for_running_run: ttl_secs must be > 0, got {ttl_secs}"
        ));
    }
    if deadman_ttl_secs <= 0 {
        return Err(anyhow!(
            "acquire_or_refresh_lease_for_running_run: deadman_ttl_secs must be > 0, got {deadman_ttl_secs}"
        ));
    }

    let mut tx = pool
        .begin()
        .await
        .context("acquire_or_refresh_lease_for_running_run: begin tx failed")?;

    let row: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT status, last_heartbeat_utc FROM runs WHERE run_id = $1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: run lock failed")?;

    let Some((status, last_heartbeat_utc)) = row else {
        tx.rollback().await.ok();
        return Err(anyhow!(
            "acquire_or_refresh_lease_for_running_run: run {run_id} not found"
        ));
    };

    if status != "RUNNING" {
        tx.rollback()
            .await
            .context("acquire_or_refresh_lease_for_running_run: rollback (not running) failed")?;
        return Ok(RunLeaseAuthorityOutcome::RunNotRunning {
            actual_status: status,
        });
    }

    let new_expiry = now_utc + Duration::seconds(ttl_secs);

    if let Some(epoch) = current_epoch {
        let refreshed: Option<(Option<Uuid>, String, i64, DateTime<Utc>)> = sqlx::query_as(
            r#"
            UPDATE runtime_leader_lease
               SET lease_expires_at = $5,
                   updated_at       = $4
             WHERE id               = 1
               AND run_id           = $1
               AND holder_id        = $2
               AND epoch            = $3
               AND lease_expires_at > $4
            RETURNING run_id, holder_id, epoch, lease_expires_at
            "#,
        )
        .bind(run_id)
        .bind(holder_id)
        .bind(epoch)
        .bind(now_utc)
        .bind(new_expiry)
        .fetch_optional(&mut *tx)
        .await
        .context("acquire_or_refresh_lease_for_running_run: refresh failed")?;

        return match refreshed {
            Some((run_id, holder_id, epoch, lease_expires_at)) => {
                tx.commit()
                    .await
                    .context("acquire_or_refresh_lease_for_running_run: commit (refresh) failed")?;
                Ok(RunLeaseAuthorityOutcome::Refreshed(RuntimeLeaderLease {
                    run_id,
                    holder_id,
                    epoch,
                    lease_expires_at,
                }))
            }
            None => {
                tx.rollback()
                    .await
                    .context("acquire_or_refresh_lease_for_running_run: rollback (lost) failed")?;
                Ok(RunLeaseAuthorityOutcome::Lost)
            }
        };
    }

    let existing: Option<(Option<Uuid>, String, i64, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT run_id, holder_id, epoch, lease_expires_at, updated_at FROM runtime_leader_lease WHERE id = 1",
    )
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: fetch existing lease failed")?;

    if let Some((existing_run_id, existing_holder, existing_epoch, existing_expires_at, existing_updated_at)) =
        &existing
    {
        let lease_raw_expired = *existing_expires_at <= now_utc;
        if !lease_raw_expired {
            tx.rollback()
                .await
                .context("acquire_or_refresh_lease_for_running_run: rollback (unexpired) failed")?;
            return Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
                run_id: *existing_run_id,
                holder_id: existing_holder.clone(),
                epoch: *existing_epoch,
                lease_expires_at: *existing_expires_at,
            }));
        }

        match existing_run_id {
            None => {
                // RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01: the
                // owning run is unknowable, so unlike the same-run branch
                // below there is no run whose heartbeat could corroborate
                // liveness. Raw lease-TTL expiry alone is therefore never
                // sufficient evidence of abandonment -- use the row's own
                // last-write timestamp as the only available liveness
                // signal, and require a full deadman window of silence
                // before treating it as orphaned. Without this, a legacy
                // lease raw-expired past the 90s RUNTIME_LEASE_TTL_SECS
                // could still belong to a runtime that remains healthy
                // within the larger 120s DEADMAN_TTL_SECS.
                let deadman_stale = now_utc
                    .signed_duration_since(*existing_updated_at)
                    .num_seconds()
                    > deadman_ttl_secs;
                if !deadman_stale {
                    tx.rollback().await.context(
                        "acquire_or_refresh_lease_for_running_run: rollback (legacy lease deadman not yet expired) failed",
                    )?;
                    return Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
                        run_id: *existing_run_id,
                        holder_id: existing_holder.clone(),
                        epoch: *existing_epoch,
                        lease_expires_at: *existing_expires_at,
                    }));
                }
            }
            Some(same_run) if *same_run == run_id => {
                let deadman_stale = match last_heartbeat_utc {
                    None => true,
                    Some(t) => now_utc.signed_duration_since(t).num_seconds() > deadman_ttl_secs,
                };
                if !deadman_stale {
                    tx.rollback().await.context(
                        "acquire_or_refresh_lease_for_running_run: rollback (deadman not yet expired) failed",
                    )?;
                    return Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
                        run_id: *existing_run_id,
                        holder_id: existing_holder.clone(),
                        epoch: *existing_epoch,
                        lease_expires_at: *existing_expires_at,
                    }));
                }
            }
            Some(other_run_id) => {
                let other_status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1")
                        .bind(other_run_id)
                        .fetch_optional(&mut *tx)
                        .await
                        .context(
                            "acquire_or_refresh_lease_for_running_run: other-run status lookup failed",
                        )?;

                if matches!(other_status.as_deref(), Some("RUNNING")) {
                    tx.rollback().await.context(
                        "acquire_or_refresh_lease_for_running_run: rollback (other run still running) failed",
                    )?;
                    return Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
                        run_id: *existing_run_id,
                        holder_id: existing_holder.clone(),
                        epoch: *existing_epoch,
                        lease_expires_at: *existing_expires_at,
                    }));
                }
                // other_run_id is durably non-RUNNING (or its row is gone)
                // and the lease is raw-expired: orphaned, safe to reclaim.
            }
        }
    }

    let acquired: Option<(Option<Uuid>, String, i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        INSERT INTO runtime_leader_lease (id, run_id, holder_id, epoch, lease_expires_at, updated_at)
        VALUES (1, $1, $2, 1, $3, $4)
        ON CONFLICT (id) DO UPDATE
          SET run_id           = excluded.run_id,
              holder_id        = excluded.holder_id,
              epoch            = runtime_leader_lease.epoch + 1,
              lease_expires_at = excluded.lease_expires_at,
              updated_at       = excluded.updated_at
        WHERE runtime_leader_lease.lease_expires_at <= $4
        RETURNING run_id, holder_id, epoch, lease_expires_at
        "#,
    )
    .bind(run_id)
    .bind(holder_id)
    .bind(new_expiry)
    .bind(now_utc)
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: acquire failed")?;

    if let Some((run_id, holder_id, epoch, lease_expires_at)) = acquired {
        tx.commit()
            .await
            .context("acquire_or_refresh_lease_for_running_run: commit (acquire) failed")?;
        return Ok(RunLeaseAuthorityOutcome::Acquired(RuntimeLeaderLease {
            run_id,
            holder_id,
            epoch,
            lease_expires_at,
        }));
    }

    let current: Option<(Option<Uuid>, String, i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT run_id, holder_id, epoch, lease_expires_at FROM runtime_leader_lease WHERE id = 1",
    )
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: fetch current lease failed")?;

    tx.rollback()
        .await
        .context("acquire_or_refresh_lease_for_running_run: rollback (held by other) failed")?;

    let (run_id, holder_id, epoch, lease_expires_at) = current.ok_or_else(|| {
        anyhow!(
            "acquire_or_refresh_lease_for_running_run: active conflict detected but lease row is missing"
        )
    })?;

    Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
        run_id,
        holder_id,
        epoch,
        lease_expires_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    async fn test_pool() -> PgPool {
        let url = std::env::var(crate::ENV_DB_URL).unwrap_or_else(|_| {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: \
                 MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
                 cargo test -p mqk-db runtime_lease -- --include-ignored"
            )
        });

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect");

        crate::migrate(&pool).await.expect("migrate");
        sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
            .execute(&pool)
            .await
            .expect("cleanup runtime_leader_lease");

        pool
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn acquire_when_no_lease_exists() {
        let pool = test_pool().await;

        let result = acquire_lease(&pool, "runtime-a", ts(1_000), 30)
            .await
            .expect("acquire");

        match result {
            LeaseAcquireOutcome::Acquired(lease) => {
                assert_eq!(lease.holder_id, "runtime-a");
                assert_eq!(lease.epoch, 1);
                assert_eq!(lease.lease_expires_at, ts(1_030));
            }
            LeaseAcquireOutcome::HeldByOther(lease) => {
                panic!("unexpected active holder: {:?}", lease)
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn second_contender_cannot_acquire_active_lease() {
        let pool = test_pool().await;

        let first = acquire_lease(&pool, "runtime-a", ts(2_000), 30)
            .await
            .expect("first acquire");
        assert!(matches!(first, LeaseAcquireOutcome::Acquired(_)));

        let second = acquire_lease(&pool, "runtime-b", ts(2_005), 30)
            .await
            .expect("second acquire");

        match second {
            LeaseAcquireOutcome::Acquired(lease) => {
                panic!("second contender unexpectedly acquired lease: {:?}", lease)
            }
            LeaseAcquireOutcome::HeldByOther(lease) => {
                assert_eq!(lease.holder_id, "runtime-a");
                assert_eq!(lease.epoch, 1);
                assert_eq!(lease.lease_expires_at, ts(2_030));
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn expired_lease_can_be_reacquired() {
        let pool = test_pool().await;

        let first = acquire_lease(&pool, "runtime-a", ts(3_000), 10)
            .await
            .expect("first acquire");
        assert!(matches!(first, LeaseAcquireOutcome::Acquired(_)));

        let second = acquire_lease(&pool, "runtime-b", ts(3_011), 10)
            .await
            .expect("second acquire after expiry");

        match second {
            LeaseAcquireOutcome::Acquired(lease) => {
                assert_eq!(lease.holder_id, "runtime-b");
                assert_eq!(lease.epoch, 2);
                assert_eq!(lease.lease_expires_at, ts(3_021));
            }
            LeaseAcquireOutcome::HeldByOther(lease) => {
                panic!("expired lease was not reacquired: {:?}", lease)
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn stale_epoch_cannot_renew() {
        let pool = test_pool().await;

        let first = acquire_lease(&pool, "runtime-a", ts(4_000), 10)
            .await
            .expect("first acquire");
        let first_epoch = match first {
            LeaseAcquireOutcome::Acquired(lease) => lease.epoch,
            LeaseAcquireOutcome::HeldByOther(lease) => {
                panic!("unexpected active holder: {:?}", lease)
            }
        };

        let stolen = acquire_lease(&pool, "runtime-b", ts(4_011), 10)
            .await
            .expect("reacquire after expiry");
        assert!(matches!(stolen, LeaseAcquireOutcome::Acquired(_)));

        let err = refresh_lease(&pool, "runtime-a", first_epoch, ts(4_012), 10)
            .await
            .expect_err("stale holder must not refresh");
        assert!(
            err.to_string().contains("lease lost"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn release_allows_new_acquire() {
        let pool = test_pool().await;

        let first = acquire_lease(&pool, "runtime-a", ts(5_000), 30)
            .await
            .expect("first acquire");
        let first_lease = match first {
            LeaseAcquireOutcome::Acquired(lease) => lease,
            LeaseAcquireOutcome::HeldByOther(lease) => {
                panic!("unexpected active holder: {:?}", lease)
            }
        };

        release_lease(&pool, &first_lease.holder_id, first_lease.epoch)
            .await
            .expect("release");

        let second = acquire_lease(&pool, "runtime-b", ts(5_001), 30)
            .await
            .expect("second acquire");

        match second {
            LeaseAcquireOutcome::Acquired(lease) => {
                assert_eq!(lease.holder_id, "runtime-b");
                assert_eq!(lease.epoch, 1);
                assert_eq!(lease.lease_expires_at, ts(5_031));
            }
            LeaseAcquireOutcome::HeldByOther(lease) => {
                panic!("released lease should be acquirable: {:?}", lease)
            }
        }
    }

    // -----------------------------------------------------------------------
    // PAPER-SOAK-STALE-CLAIM-RECOVERY-03: acquire_or_refresh_lease_for_running_run
    // -----------------------------------------------------------------------

    async fn make_run_with_status(pool: &PgPool, status: &str) -> uuid::Uuid {
        let run_id = uuid::Uuid::new_v4(); // allow: test-only — isolated DB test fixture, never called from production paths
        let fixture_ts = ts(0);
        crate::insert_run(
            pool,
            &crate::NewRun {
                run_id,
                engine_id: format!("runtime-lease-test-{run_id}"),
                mode: "PAPER".to_string(),
                started_at_utc: fixture_ts,
                git_hash: "TEST".to_string(),
                config_hash: format!("cfg-{run_id}"),
                config_json: serde_json::json!({}),
                host_fingerprint: "TESTHOST".to_string(),
            },
        )
        .await
        .expect("insert_run");
        match status {
            "CREATED" => {}
            "ARMED" => {
                crate::arm_run(pool, run_id).await.expect("arm_run");
            }
            "RUNNING" => {
                crate::arm_run(pool, run_id).await.expect("arm_run");
                crate::begin_run(pool, run_id).await.expect("begin_run");
            }
            "HALTED" => {
                crate::halt_run(pool, run_id, fixture_ts)
                    .await
                    .expect("halt_run");
            }
            "STOPPED" => {
                crate::arm_run(pool, run_id).await.expect("arm_run");
                crate::begin_run(pool, run_id).await.expect("begin_run");
                crate::halt_run(pool, run_id, fixture_ts)
                    .await
                    .expect("halt_run");
                crate::clear_halted_run(pool, run_id)
                    .await
                    .expect("clear_halted_run");
            }
            other => panic!("make_run_with_status: unsupported status {other}"),
        }
        run_id
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_acquire_succeeds_on_running_run() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            ts(10_000),
            30,
            30,
        )
        .await
        .expect("acquire");

        match outcome {
            RunLeaseAuthorityOutcome::Acquired(lease) => {
                assert_eq!(lease.run_id, Some(run_id));
                assert_eq!(lease.holder_id, "runtime-a");
                assert_eq!(lease.epoch, 1);
            }
            other => panic!("expected Acquired, got {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_refresh_succeeds_for_same_holder_epoch_on_running_run() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        let first = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            ts(11_000),
            30,
            30,
        )
        .await
        .expect("acquire");
        let epoch = match first {
            RunLeaseAuthorityOutcome::Acquired(lease) => lease.epoch,
            other => panic!("expected Acquired, got {other:?}"),
        };

        let second = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            Some(epoch),
            ts(11_010),
            30,
            30,
        )
        .await
        .expect("refresh");
        match second {
            RunLeaseAuthorityOutcome::Refreshed(lease) => {
                assert_eq!(lease.run_id, Some(run_id));
                assert_eq!(lease.holder_id, "runtime-a");
                assert_eq!(lease.epoch, epoch);
                assert_eq!(lease.lease_expires_at, ts(11_040));
            }
            other => panic!("expected Refreshed, got {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_second_contender_cannot_acquire_active_lease() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-a", None, ts(12_000), 30, 30,
        )
        .await
        .expect("first acquire");

        let second = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-b",
            None,
            ts(12_005),
            30,
            30,
        )
        .await
        .expect("second attempt");
        match second {
            RunLeaseAuthorityOutcome::HeldByOther(current) => {
                assert_eq!(current.run_id, Some(run_id));
                assert_eq!(current.holder_id, "runtime-a");
            }
            other => panic!("expected HeldByOther, got {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_refuses_on_halted_run() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "HALTED").await;

        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            ts(13_000),
            30,
            30,
        )
        .await
        .expect("call must not error");
        assert_eq!(
            outcome,
            RunLeaseAuthorityOutcome::RunNotRunning {
                actual_status: "HALTED".to_string()
            }
        );
        let lease = fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease");
        assert!(
            lease.is_none(),
            "S05: a HALTED run must not be able to acquire a new lease"
        );
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_refuses_on_stopped_run_and_does_not_touch_lease() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "STOPPED").await;

        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            ts(14_000),
            30,
            30,
        )
        .await
        .expect("call must not error");
        assert_eq!(
            outcome,
            RunLeaseAuthorityOutcome::RunNotRunning {
                actual_status: "STOPPED".to_string()
            }
        );
        let lease = fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease");
        assert!(
            lease.is_none(),
            "S06: a STOPPED run must not be able to acquire a new lease"
        );
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn run_aware_refresh_lost_on_expired_epoch_while_still_running() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        let first = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            None,
            ts(15_000),
            5,
            5,
        )
        .await
        .expect("acquire");
        let epoch = match first {
            RunLeaseAuthorityOutcome::Acquired(lease) => lease.epoch,
            other => panic!("expected Acquired, got {other:?}"),
        };

        // Past expiry, but the run is still RUNNING -- this must surface as
        // `Lost` (genuine contest/loss), not `RunNotRunning`.
        let refreshed = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-a",
            Some(epoch),
            ts(15_011),
            5,
            5,
        )
        .await
        .expect("refresh attempt must not error");
        assert_eq!(refreshed, RunLeaseAuthorityOutcome::Lost);
    }

    // -------------------------------------------------------------------
    // RUNTIME-LEASE-RUN-IDENTITY-AUTHORITY-01: run-bound cross-run
    // reconciliation -- the negative controls proving the P6 defect (a
    // different run's stale lease judged by the NEW run's own heartbeat) is
    // closed, plus the restored same-run deadman reconciliation.
    // -------------------------------------------------------------------

    /// RED proof: reproduces the exact rejected-P6 failure mode using ONLY
    /// evidence available in this test (no reliance on the old code path,
    /// which no longer exists) -- a genuinely different, already-STOPPED
    /// run's stale lease must never be judged using the NEW run's own fresh
    /// heartbeat. If this reconciliation regressed to comparing the wrong
    /// run's heartbeat, a fresh `run_b` heartbeat would make the lease look
    /// "still owned by a live holder" and this acquisition would wrongly
    /// return `HeldByOther` forever -- a permanent lockout for run_b.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr01_red_new_run_fresh_heartbeat_is_never_evidence_about_old_runs_lease() {
        let pool = test_pool().await;

        // run_a: acquires the lease, then is stopped WITHOUT the lease being
        // deleted -- exactly what `stop_run_if_evidence_clean` produces in
        // production (Layer A cleanup is `clear_halted_run_and_reset_stale_
        // claims`-only; the RUNNING/ARMED -> STOPPED path never deletes it).
        let run_a = make_run_with_status(&pool, "RUNNING").await;
        acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(30_000), 90, 120,
        )
        .await
        .expect("run_a acquire")
        .expect_acquired();
        sqlx::query("UPDATE runs SET status = 'STOPPED', stopped_at_utc = $2 WHERE run_id = $1")
            .bind(run_a)
            .bind(ts(30_050))
            .execute(&pool)
            .await
            .expect("force run_a to STOPPED (simulates stop_run_if_evidence_clean, which never deletes the lease)");

        // run_b: a brand-new, different run. Its heartbeat is set FRESH,
        // exactly like `start_runtime_effects`' initial `heartbeat_run` call
        // before the first tick -- ts(30_101), only 1s before this
        // acquisition attempt.
        let run_b = make_run_with_status(&pool, "RUNNING").await;
        crate::heartbeat_run(&pool, run_b, ts(30_101))
            .await
            .expect("run_b initial heartbeat");

        // 121s after run_a's lease was acquired with a 90s TTL: raw-expired.
        // run_b's own heartbeat is 1s old -- if it were (wrongly) used as
        // evidence about run_a's holder, deadman_stale would evaluate false
        // and this would return HeldByOther, permanently blocking run_b.
        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(30_121), 90, 120,
        )
        .await
        .expect("run_b acquire must not error");

        match outcome {
            RunLeaseAuthorityOutcome::Acquired(lease) => {
                assert_eq!(
                    lease.run_id,
                    Some(run_b),
                    "run_b must acquire its own lease, bound to its own run_id"
                );
                assert_eq!(lease.holder_id, "runtime-b");
            }
            other => panic!(
                "run_a's stopped, orphaned lease must not block run_b using run_b's own \
                 fresh heartbeat as false evidence -- got {other:?}"
            ),
        }
    }

    /// Negative control 9 (explicit heartbeat-source proof): even when
    /// run_b's heartbeat is set to look ARBITRARILY fresh (identical to
    /// `now_utc`), a different, STOPPED run_a's raw-expired lease is still
    /// reclaimed -- proving disposition never depends on run_b's heartbeat
    /// value at all, in either direction.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr02_different_run_reclaim_is_independent_of_new_runs_heartbeat_value() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(31_000), 90, 120,
        )
        .await
        .expect("run_a acquire")
        .expect_acquired();
        sqlx::query("UPDATE runs SET status = 'STOPPED', stopped_at_utc = $2 WHERE run_id = $1")
            .bind(run_a)
            .bind(ts(31_050))
            .execute(&pool)
            .await
            .expect("force run_a to STOPPED");

        let run_b = make_run_with_status(&pool, "RUNNING").await;
        // Heartbeat set to the EXACT instant of the acquisition attempt --
        // maximally fresh, the strongest possible false signal if it were
        // (wrongly) consulted.
        crate::heartbeat_run(&pool, run_b, ts(31_121))
            .await
            .expect("run_b heartbeat");

        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(31_121), 90, 120,
        )
        .await
        .expect("run_b acquire must not error");
        assert!(
            matches!(outcome, RunLeaseAuthorityOutcome::Acquired(_)),
            "expected Acquired regardless of run_b's heartbeat freshness, got {outcome:?}"
        );
    }

    /// Negative control 11: if the OTHER run somehow still reports RUNNING
    /// (structurally should never happen given `create_or_reuse_run_for_start`'s
    /// single-active-run invariant, but the reconciliation must not assume
    /// that invariant holds), the new contender fails closed.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr03_different_run_still_running_fails_closed() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(32_000), 90, 120,
        )
        .await
        .expect("run_a acquire")
        .expect_acquired();
        // Force the lease raw-expired WITHOUT changing run_a's status --
        // constructs the adversarial "other run still RUNNING" case directly
        // via SQL, since the normal admission path cannot produce it.
        sqlx::query("UPDATE runtime_leader_lease SET lease_expires_at = $1 WHERE id = 1")
            .bind(ts(32_001))
            .execute(&pool)
            .await
            .expect("force lease raw-expired");

        let run_b = make_run_with_status(&pool, "RUNNING").await;
        crate::heartbeat_run(&pool, run_b, ts(32_121))
            .await
            .expect("run_b heartbeat");

        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(32_121), 90, 120,
        )
        .await
        .expect("run_b acquire attempt must not error");
        match outcome {
            RunLeaseAuthorityOutcome::HeldByOther(current) => {
                assert_eq!(current.run_id, Some(run_a));
            }
            other => panic!("expected HeldByOther (fail closed), got {other:?}"),
        }
    }

    /// Negative control 3: a refresh attempt whose `run_id` argument does not
    /// match the lease's bound run must fail the CAS exactly like any other
    /// mismatch (holder/epoch) -- reported as ordinary `Lost`.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr04_different_run_refresh_fails() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        let first = acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(33_000), 90, 120,
        )
        .await
        .expect("run_a acquire");
        let epoch = first.expect_acquired().epoch;

        // A second, different RUNNING run attempts to "refresh" using run_a's
        // own holder_id/epoch but its OWN run_id -- must not succeed.
        let run_b = make_run_with_status(&pool, "RUNNING").await;
        let refreshed = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_b,
            "runtime-a",
            Some(epoch),
            ts(33_001),
            90,
            120,
        )
        .await
        .expect("refresh attempt must not error");
        assert_eq!(refreshed, RunLeaseAuthorityOutcome::Lost);

        // Sanity: run_a's real lease is completely untouched.
        let lease = fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease")
            .expect("lease row must still exist");
        assert_eq!(lease.run_id, Some(run_a));
        assert_eq!(lease.holder_id, "runtime-a");
        assert_eq!(lease.epoch, epoch);
    }

    /// Negative control 4: `release_lease_for_run` must not delete a lease
    /// bound to a different run, even with the exact same holder_id/epoch
    /// values (a holder_id collision across runs should never happen in
    /// practice, but the delete predicate must not rely on that).
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr05_different_run_release_cannot_delete_current_lease() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        let first = acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(34_000), 90, 120,
        )
        .await
        .expect("run_a acquire");
        let epoch = first.expect_acquired().epoch;

        let run_b = uuid::Uuid::new_v4(); // allow: test-only — never a real run_id
        release_lease_for_run(&pool, run_b, "runtime-a", epoch)
            .await
            .expect("release_lease_for_run must not error even on a non-matching run_id");

        let lease = fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease")
            .expect("run_a's lease must survive a different-run release attempt");
        assert_eq!(lease.run_id, Some(run_a));
        assert_eq!(lease.holder_id, "runtime-a");
        assert_eq!(lease.epoch, epoch);
    }

    /// Negative control 5: `verify_lease_for_run` must not validate a lease
    /// bound to a different run.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr06_different_run_verification_fails() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        let first = acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(35_000), 90, 120,
        )
        .await
        .expect("run_a acquire");
        let epoch = first.expect_acquired().epoch;

        let run_b = uuid::Uuid::new_v4(); // allow: test-only — never a real run_id
        let same_run_ok = verify_lease_for_run(&pool, run_a, "runtime-a", epoch, ts(35_001))
            .await
            .expect("verify must not error");
        assert!(same_run_ok, "the true owning run must verify successfully");

        let cross_run_ok = verify_lease_for_run(&pool, run_b, "runtime-a", epoch, ts(35_001))
            .await
            .expect("verify must not error");
        assert!(
            !cross_run_ok,
            "a different run must never validate another run's lease authority"
        );
    }

    /// Negative control 7: same-run raw-expired but deadman-fresh takeover is
    /// refused -- the restored, correctly-scoped intent of
    /// DEADMAN-LEASE-TTL-RECONCILE-01.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr07_same_run_lease_expired_but_deadman_fresh_refuses_steal() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;
        crate::heartbeat_run(&pool, run_id, ts(36_000))
            .await
            .expect("heartbeat_run");

        acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-a", None, ts(36_000), 90, 120,
        )
        .await
        .expect("acquire")
        .expect_acquired();

        // 100s later: the 90s lease is raw-expired, but the heartbeat is
        // only 100s old -- still within the 120s deadman TTL.
        let steal_attempt = acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-b", None, ts(36_100), 90, 120,
        )
        .await
        .expect("steal attempt must not error");
        match steal_attempt {
            RunLeaseAuthorityOutcome::HeldByOther(current) => {
                assert_eq!(current.run_id, Some(run_id));
                assert_eq!(current.holder_id, "runtime-a");
            }
            other => panic!(
                "expected HeldByOther (deadman not yet expired must block the steal), got {other:?}"
            ),
        }
    }

    /// Negative control 8: same-run raw-expired AND deadman-expired takeover
    /// succeeds -- the reconciliation gate must not become a permanent
    /// lockout once both signals genuinely agree.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr08_same_run_lease_expired_and_deadman_expired_permits_steal() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;
        crate::heartbeat_run(&pool, run_id, ts(37_000))
            .await
            .expect("heartbeat_run");

        acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-a", None, ts(37_000), 90, 120,
        )
        .await
        .expect("acquire")
        .expect_acquired();

        // 121s later: both the 90s lease AND the 120s deadman window have
        // elapsed since the only heartbeat/refresh this run ever received.
        let steal_attempt = acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-b", None, ts(37_121), 90, 120,
        )
        .await
        .expect("steal attempt must not error");
        match steal_attempt {
            RunLeaseAuthorityOutcome::Acquired(lease) => {
                assert_eq!(lease.run_id, Some(run_id));
                assert_eq!(lease.holder_id, "runtime-b");
                assert_eq!(lease.epoch, 2);
            }
            other => panic!("expected Acquired once both signals agree, got {other:?}"),
        }
    }

    /// Negative control 10: STOPPED old run + expired old-run lease -> the
    /// new run can eventually acquire without permanent lockout (a direct
    /// restatement of cr01/cr02, phrased as the mission's own numbered
    /// control for traceability).
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr09_stopped_old_run_never_permanently_locks_out_new_run() {
        let pool = test_pool().await;

        let run_a = make_run_with_status(&pool, "RUNNING").await;
        acquire_or_refresh_lease_for_running_run(
            &pool, run_a, "runtime-a", None, ts(38_000), 90, 120,
        )
        .await
        .expect("run_a acquire")
        .expect_acquired();
        sqlx::query("UPDATE runs SET status = 'STOPPED', stopped_at_utc = $2 WHERE run_id = $1")
            .bind(run_a)
            .bind(ts(38_050))
            .execute(&pool)
            .await
            .expect("force run_a to STOPPED");

        let run_b = make_run_with_status(&pool, "RUNNING").await;
        // Deliberately NO heartbeat_run call for run_b -- last_heartbeat_utc
        // is NULL, the most adversarial case for a same-run deadman check,
        // proving this path never reaches that check at all for a
        // different-run lease.
        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(38_091), 90, 120,
        )
        .await
        .expect("run_b acquire must not error");
        assert!(
            matches!(outcome, RunLeaseAuthorityOutcome::Acquired(_)),
            "expected Acquired, got {outcome:?}"
        );
    }

    /// Negative control 12 (RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01
    /// RED proof, restated GREEN): a row with `run_id IS NULL` (simulating a
    /// pre-migration-0068 row -- inserted directly via SQL, bypassing every
    /// Rust writer) that is raw-expired (past its 90s lease TTL) but still
    /// deadman-fresh (its own `updated_at` is inside the 120s deadman
    /// window) must NOT be reclaimable. Before this patch, the legacy branch
    /// treated raw expiry alone as authoritative and this scenario returned
    /// `Acquired` -- exactly the unsafe transition the mission describes: a
    /// legacy NULL lease raw-expiring while the runtime that holds it
    /// remains deadman-healthy. A fresh run_b heartbeat is used deliberately
    /// so a wrong fall-through into the same-run deadman path (which treats
    /// a fresh heartbeat as evidence of liveness) would be caught by the
    /// same correct refusal, not mask the bug.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr10_legacy_null_run_id_row_deadman_fresh_blocks_takeover() {
        let pool = test_pool().await;

        sqlx::query(
            r#"
            INSERT INTO runtime_leader_lease (id, run_id, holder_id, epoch, lease_expires_at, updated_at)
            VALUES (1, NULL, 'legacy-holder', 1, $1, $2)
            "#,
        )
        .bind(ts(39_090)) // lease_expires_at = updated_at + 90s (RUNTIME_LEASE_TTL_SECS)
        .bind(ts(39_000)) // updated_at
        .execute(&pool)
        .await
        .expect("seed legacy unversioned lease row");

        let run_b = make_run_with_status(&pool, "RUNNING").await;
        crate::heartbeat_run(&pool, run_b, ts(39_100))
            .await
            .expect("run_b heartbeat");

        // now=39_100: 100s since the legacy row's updated_at -- past its 90s
        // lease TTL (raw-expired) but inside the 120s deadman window.
        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(39_100), 90, 120,
        )
        .await
        .expect("run_b acquire must not error");
        match outcome {
            RunLeaseAuthorityOutcome::HeldByOther(current) => {
                assert_eq!(current.run_id, None);
            }
            other => panic!(
                "a raw-expired but deadman-fresh legacy NULL-run_id row must block takeover, got {other:?}"
            ),
        }
    }

    /// Companion to `cr10`: once the same legacy row's `updated_at` is also
    /// past the 120s deadman window, it is not a permanent lockout -- the
    /// same reconciliation that blocks the fresh case permits reclaim once
    /// both signals genuinely agree, exactly like the same-run case (cr08).
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr10b_legacy_null_run_id_row_deadman_stale_permits_takeover() {
        let pool = test_pool().await;

        sqlx::query(
            r#"
            INSERT INTO runtime_leader_lease (id, run_id, holder_id, epoch, lease_expires_at, updated_at)
            VALUES (1, NULL, 'legacy-holder', 1, $1, $2)
            "#,
        )
        .bind(ts(40_090)) // lease_expires_at = updated_at + 90s
        .bind(ts(40_000)) // updated_at
        .execute(&pool)
        .await
        .expect("seed legacy unversioned lease row");

        let run_b = make_run_with_status(&pool, "RUNNING").await;
        // Deliberately no heartbeat for run_b -- proving disposition never
        // depends on the new run's heartbeat state for the legacy branch.
        // now=40_121: 121s since the legacy row's updated_at, past both the
        // 90s lease TTL and the 120s deadman window.
        let outcome = acquire_or_refresh_lease_for_running_run(
            &pool, run_b, "runtime-b", None, ts(40_121), 90, 120,
        )
        .await
        .expect("run_b acquire must not error");
        match outcome {
            RunLeaseAuthorityOutcome::Acquired(lease) => {
                assert_eq!(lease.run_id, Some(run_b));
            }
            other => panic!(
                "a raw-expired AND deadman-stale legacy NULL-run_id row must be reclaimable, got {other:?}"
            ),
        }
    }

    /// FK delete-action review (RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-
    /// SAFETY-01): migration 0069 changes `runtime_leader_lease_run_id_fkey`
    /// from ON DELETE CASCADE to ON DELETE RESTRICT. Proves the production
    /// invariant directly: a run row that a leadership lease still durably
    /// points to must refuse deletion, not silently cascade the lease away.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn fk01_run_delete_restricted_while_runtime_lease_references_it() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-a", None, ts(41_000), 90, 120,
        )
        .await
        .expect("acquire")
        .expect_acquired();

        let err = sqlx::query("DELETE FROM runs WHERE run_id = $1")
            .bind(run_id)
            .execute(&pool)
            .await
            .expect_err("deleting a run with live runtime_leader_lease authority must be refused");
        assert!(
            err.to_string().to_lowercase().contains("foreign key"),
            "expected a foreign key violation, got: {err}"
        );

        let lease = fetch_current_lease(&pool)
            .await
            .expect("fetch_current_lease")
            .expect("the lease must survive the refused delete");
        assert_eq!(lease.run_id, Some(run_id));
    }

    /// Negative control 13 (fencing intact): the same run-row serialization
    /// boundary `clear_halted_run_and_reset_stale_claims` relies on is still
    /// exercised correctly end-to-end for a run-bound lease -- a HALTED run
    /// with an unexpired run-bound lease still blocks `clear-halted-run`,
    /// proving the run_id column addition did not weaken that fence.
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn cr11_halted_run_with_unexpired_run_bound_lease_still_blocks_clear() {
        let pool = test_pool().await;
        let run_id = make_run_with_status(&pool, "RUNNING").await;

        acquire_or_refresh_lease_for_running_run(
            &pool, run_id, "runtime-a", None, ts(40_000), 300, 300,
        )
        .await
        .expect("acquire")
        .expect_acquired();

        crate::halt_run(&pool, run_id, ts(40_010))
            .await
            .expect("halt_run");

        let outcome = crate::clear_halted_run_and_reset_stale_claims(&pool, run_id, ts(40_020))
            .await
            .expect("clear attempt must not error");
        assert!(
            matches!(
                outcome,
                crate::ClearHaltedRunOutcome::ActiveRuntimeLease { .. }
            ),
            "an unexpired run-bound lease must still block clear-halted-run, got {outcome:?}"
        );
    }

    trait ExpectAcquired {
        fn expect_acquired(self) -> RuntimeLeaderLease;
    }

    impl ExpectAcquired for RunLeaseAuthorityOutcome {
        fn expect_acquired(self) -> RuntimeLeaderLease {
            match self {
                RunLeaseAuthorityOutcome::Acquired(lease) => lease,
                other => panic!("expected Acquired, got {other:?}"),
            }
        }
    }
}
