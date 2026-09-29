//! B6 final correction (migration 0088): durable broker-account authority.
//!
//! Economic evidence (fees, options-lifecycle activity, cursors, applied
//! adjustments) is scoped by the PROVIDER's own account id, never by the API
//! credential. A credential rotation on the same provider account resolves to
//! the same [`BrokerAccountAuthority::key`]; two provider accounts never share
//! one. The daemon registers an authority only after the authenticated
//! provider account endpoint proved the id; every account-scoped table
//! references `sys_broker_account_authority` (0088), so an unregistered scope
//! cannot be written.
//!
//! Legacy rows written before 0088 (keyed by an API key id) are retained as
//! legacy-unverified: no authority row can name them, so they never satisfy an
//! authoritative scoped read, cursor or apply. [`count_legacy_unverified_account_rows`]
//! reports them for the operator. Nothing is ever re-labelled or inferred from
//! a credential.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Account-scoped tables whose `broker_account_id` must be a registered
/// authority key (0088).
pub const ACCOUNT_SCOPED_TABLES: [&str; 5] = [
    "sys_crypto_fee_activity_ledger",
    "sys_crypto_fee_ingestion_cursor",
    "sys_option_lifecycle_activity_ledger",
    "sys_option_lifecycle_ingestion_cursor",
    "sys_option_lifecycle_applied",
];

/// Typed provider-account identity: `broker`, the provider's own account id and
/// the deployment mode the account was verified under. Constructed only
/// through [`BrokerAccountAuthority::new`], which canonicalizes and validates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BrokerAccountAuthority {
    broker: String,
    provider_account_id: String,
    deployment_mode: String,
}

fn canonical_token(what: &str, raw: &str, allow_dot_underscore: bool) -> Result<String> {
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() {
        bail!("broker_account_authority: {what} must not be blank");
    }
    let ok = t.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '-' || (allow_dot_underscore && (c == '.' || c == '_'))
    });
    if !ok || t.len() > 128 {
        bail!("broker_account_authority: {what} {raw:?} is not a canonical token");
    }
    Ok(t)
}

impl BrokerAccountAuthority {
    pub fn new(broker: &str, provider_account_id: &str, deployment_mode: &str) -> Result<Self> {
        Ok(Self {
            broker: canonical_token("broker", broker, false)?,
            provider_account_id: canonical_token("provider_account_id", provider_account_id, true)?,
            deployment_mode: canonical_token("deployment_mode", deployment_mode, true)?,
        })
    }

    pub fn broker(&self) -> &str {
        &self.broker
    }

    pub fn provider_account_id(&self) -> &str {
        &self.provider_account_id
    }

    pub fn deployment_mode(&self) -> &str {
        &self.deployment_mode
    }

    /// The canonical economic-account key stored in every account-scoped
    /// table's `broker_account_id`: `{broker}:{provider_account_id}`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.broker, self.provider_account_id)
    }
}

/// Register the authority if new, or confirm an existing registration is
/// identical. A registration that names the same provider account under a
/// different deployment mode refuses (whole call, zero mutation): one provider
/// account cannot be both Paper and Live.
pub async fn verify_or_register_broker_account_authority(
    pool: &PgPool,
    authority: &BrokerAccountAuthority,
    now_utc: DateTime<Utc>,
) -> Result<()> {
    let key = authority.key();
    sqlx::query(
        r#"
        insert into sys_broker_account_authority (
            authority_key, broker, provider_account_id, deployment_mode, first_verified_at_utc
        ) values ($1, $2, $3, $4, $5)
        on conflict (authority_key) do nothing
        "#,
    )
    .bind(&key)
    .bind(&authority.broker)
    .bind(&authority.provider_account_id)
    .bind(&authority.deployment_mode)
    .bind(now_utc)
    .execute(pool)
    .await
    .context("verify_or_register_broker_account_authority: insert failed")?;

    let registered = fetch_broker_account_authority(pool, &key)
        .await?
        .context("verify_or_register_broker_account_authority: row vanished after insert")?;
    if registered != *authority {
        bail!(
            "broker_account_authority: refused -- {key} is registered under deployment_mode \
             {:?}, not {:?}",
            registered.deployment_mode,
            authority.deployment_mode
        );
    }
    Ok(())
}

pub async fn fetch_broker_account_authority(
    pool: &PgPool,
    key: &str,
) -> Result<Option<BrokerAccountAuthority>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        r#"
        select broker, provider_account_id, deployment_mode
          from sys_broker_account_authority
         where authority_key = $1
        "#,
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .context("fetch_broker_account_authority failed")?;
    row.map(|(b, p, m)| BrokerAccountAuthority::new(&b, &p, &m))
        .transpose()
}

/// Rows in each account-scoped table whose `broker_account_id` no registered
/// authority names -- legacy-unverified evidence retained by 0088. Ordered by
/// [`ACCOUNT_SCOPED_TABLES`].
pub async fn count_legacy_unverified_account_rows(pool: &PgPool) -> Result<Vec<(String, i64)>> {
    let mut out = Vec::with_capacity(ACCOUNT_SCOPED_TABLES.len());
    for table in ACCOUNT_SCOPED_TABLES {
        // `table` is a compile-time constant from ACCOUNT_SCOPED_TABLES.
        let sql = format!(
            "select count(*) from {table} t where not exists \
             (select 1 from sys_broker_account_authority a where a.authority_key = t.broker_account_id)"
        );
        let (n,): (i64,) = sqlx::query_as(&sql)
            .fetch_one(pool)
            .await
            .with_context(|| format!("count_legacy_unverified_account_rows({table}) failed"))?;
        out.push((table.to_string(), n));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_broker_and_provider_account_never_a_credential() {
        let a = BrokerAccountAuthority::new(
            " Alpaca ",
            "E6FE16F3-64A4-4921-8928-CADF02F92F98",
            "Paper",
        )
        .unwrap();
        assert_eq!(a.key(), "alpaca:e6fe16f3-64a4-4921-8928-cadf02f92f98");
        assert_eq!(a.deployment_mode(), "paper");
    }

    #[test]
    fn blank_or_non_canonical_components_are_refused() {
        for (b, p, m) in [
            ("", "acct", "paper"),
            ("alpaca", "  ", "paper"),
            ("alpaca", "acct", ""),
            ("alpaca", "has space", "paper"),
            ("alpaca", "colon:injected", "paper"),
            ("al:paca", "acct", "paper"),
        ] {
            assert!(
                BrokerAccountAuthority::new(b, p, m).is_err(),
                "{b:?}/{p:?}/{m:?}"
            );
        }
    }
}
