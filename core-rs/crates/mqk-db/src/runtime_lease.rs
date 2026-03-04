use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents the single-row leader lease record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeaderLease {
    pub holder_id: String,
    pub epoch: i64,
    pub lease_expires_at: DateTime<Utc>,
}

/// Acquire leadership by upserting the single-row lease.
///
/// SAFETY: This must be transactional and single-flight.
/// A proper implementation should:
/// - compare-and-swap on lease_expires_at (only acquire if expired)
/// - increment epoch atomically upon acquisition
/// - return current lease holder if not acquired
///
/// This file is a scaffold only and is NOT wired in.
pub async fn acquire_lease_scaffold(
    _db: &sqlx::PgPool,
    _holder_id: &str,
    _lease_ttl_seconds: i64,
) -> anyhow::Result<RuntimeLeaderLease> {
    anyhow::bail!("acquire_lease_scaffold: not implemented (scaffold)")
}

/// Renew lease if you still hold it.
pub async fn renew_lease_scaffold(
    _db: &sqlx::PgPool,
    _holder_id: &str,
    _epoch: i64,
    _lease_ttl_seconds: i64,
) -> anyhow::Result<RuntimeLeaderLease> {
    anyhow::bail!("renew_lease_scaffold: not implemented (scaffold)")
}

/// Release lease (best-effort). Many systems prefer "let it expire" instead.
pub async fn release_lease_scaffold(
    _db: &sqlx::PgPool,
    _holder_id: &str,
    _epoch: i64,
) -> anyhow::Result<()> {
    anyhow::bail!("release_lease_scaffold: not implemented (scaffold)")
}
