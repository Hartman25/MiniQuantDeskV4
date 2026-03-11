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
}
