//! B6 (fee economic consumer): confirmed Crypto broker fees reach the
//! canonical mutable ledger.
//!
//! Alpaca posts crypto fees once per day as separate `CFEE`/`FEE` account
//! activities, never inside a fill, so a fill-derived `PortfolioState` alone
//! overstates cash by every confirmed fee. Run recovery therefore replays the
//! durable fee ledger (`sys_crypto_fee_activity_ledger`, ingested by
//! `crypto_fee_ingestion`) into the ledger as ordinary
//! `LedgerEntry::Cash` entries whose amount is the provider's SIGNED fee
//! exactly as reported.
//!
//! - Idempotent by construction: recovery is a pure fold over immutable rows
//!   keyed `(account, activity_id)`; a rebuild replays the same rows and
//!   yields the same cash, never a double charge.
//! - Scoped: only accounts registered under this deployment mode, only the
//!   Crypto24_7 domain's ledger.
//! - No invented attribution: an asset-denominated fee is NOT converted to
//!   cash (no per-fill price evidence); it is reported as `unattributed` and
//!   the returned summary says cost truth is partial. Absence of any fee
//!   evidence is likewise never reported as proven-zero cost
//!   ([`CryptoFeeEvidenceSummary::cost_fully_attributed`]).

use sqlx::PgPool;

use mqk_db::CryptoFeeEvidenceSummary;
use mqk_portfolio::{apply_entry, CashEntry, LedgerEntry, PortfolioState};

/// Replay confirmed cash fees into `portfolio`; returns the evidence summary
/// (including how much fee evidence could NOT be attributed to cash).
pub async fn replay_crypto_fees_into_portfolio(
    pool: &PgPool,
    deployment_mode: &str,
    portfolio: &mut PortfolioState,
) -> anyhow::Result<CryptoFeeEvidenceSummary> {
    let summary = mqk_db::summarize_crypto_fee_evidence(pool, deployment_mode).await?;
    for fee in mqk_db::list_confirmed_crypto_cash_fees(pool, deployment_mode).await? {
        apply_entry(
            portfolio,
            LedgerEntry::Cash(CashEntry::new(
                fee.fee_micros,
                format!("crypto_fee:{}:{}", fee.broker_account_id, fee.activity_id),
            )),
        );
    }
    Ok(summary)
}
