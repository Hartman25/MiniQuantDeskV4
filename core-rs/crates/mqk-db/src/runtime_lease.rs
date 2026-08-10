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

/// The single runtime leader lease row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeLeaderLease {
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

/// Read the current lease row, if present.
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

    Ok(
        row.map(|(holder_id, epoch, lease_expires_at)| RuntimeLeaderLease {
            holder_id,
            epoch,
            lease_expires_at,
        }),
    )
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
/// the two unfenced primitives.
pub async fn acquire_or_refresh_lease_for_running_run(
    pool: &PgPool,
    run_id: Uuid,
    holder_id: &str,
    current_epoch: Option<i64>,
    now_utc: DateTime<Utc>,
    ttl_secs: i64,
) -> anyhow::Result<RunLeaseAuthorityOutcome> {
    if ttl_secs <= 0 {
        return Err(anyhow!(
            "acquire_or_refresh_lease_for_running_run: ttl_secs must be > 0, got {ttl_secs}"
        ));
    }

    let mut tx = pool
        .begin()
        .await
        .context("acquire_or_refresh_lease_for_running_run: begin tx failed")?;

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .context("acquire_or_refresh_lease_for_running_run: run lock failed")?;

    let Some(status) = status else {
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
        .fetch_optional(&mut *tx)
        .await
        .context("acquire_or_refresh_lease_for_running_run: refresh failed")?;

        return match refreshed {
            Some((holder_id, epoch, lease_expires_at)) => {
                tx.commit()
                    .await
                    .context("acquire_or_refresh_lease_for_running_run: commit (refresh) failed")?;
                Ok(RunLeaseAuthorityOutcome::Refreshed(RuntimeLeaderLease {
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
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: acquire failed")?;

    if let Some((holder_id, epoch, lease_expires_at)) = acquired {
        tx.commit()
            .await
            .context("acquire_or_refresh_lease_for_running_run: commit (acquire) failed")?;
        return Ok(RunLeaseAuthorityOutcome::Acquired(RuntimeLeaderLease {
            holder_id,
            epoch,
            lease_expires_at,
        }));
    }

    let current: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT holder_id, epoch, lease_expires_at FROM runtime_leader_lease WHERE id = 1",
    )
    .fetch_optional(&mut *tx)
    .await
    .context("acquire_or_refresh_lease_for_running_run: fetch current lease failed")?;

    tx.rollback()
        .await
        .context("acquire_or_refresh_lease_for_running_run: rollback (held by other) failed")?;

    let (holder_id, epoch, lease_expires_at) = current.ok_or_else(|| {
        anyhow!(
            "acquire_or_refresh_lease_for_running_run: active conflict detected but lease row is missing"
        )
    })?;

    Ok(RunLeaseAuthorityOutcome::HeldByOther(RuntimeLeaderLease {
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
        )
        .await
        .expect("acquire");

        match outcome {
            RunLeaseAuthorityOutcome::Acquired(lease) => {
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
        )
        .await
        .expect("refresh");
        match second {
            RunLeaseAuthorityOutcome::Refreshed(lease) => {
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

        acquire_or_refresh_lease_for_running_run(&pool, run_id, "runtime-a", None, ts(12_000), 30)
            .await
            .expect("first acquire");

        let second = acquire_or_refresh_lease_for_running_run(
            &pool,
            run_id,
            "runtime-b",
            None,
            ts(12_005),
            30,
        )
        .await
        .expect("second attempt");
        match second {
            RunLeaseAuthorityOutcome::HeldByOther(current) => {
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
        )
        .await
        .expect("refresh attempt must not error");
        assert_eq!(refreshed, RunLeaseAuthorityOutcome::Lost);
    }
}
