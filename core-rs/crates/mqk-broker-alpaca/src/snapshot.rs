//! AP-03: Alpaca broker snapshot normalization helpers.
//!
//! Pure, deterministic functions for mapping Alpaca REST wire responses into
//! the canonical [`mqk_schemas::BrokerSnapshot`] shape.
//!
//! # Endpoints consumed by `fetch_broker_snapshot`
//!
//! | Endpoint                         | Canonical target         |
//! |----------------------------------|--------------------------|
//! | `GET /v2/account`                | `BrokerAccount`          |
//! | `GET /v2/positions`              | `Vec<BrokerPosition>`    |
//! | `GET /v2/orders?status=open`     | `Vec<BrokerOrder>`       |
//!
//! # Fills
//!
//! Recent fills are **not** included in the snapshot produced here.
//! The Alpaca REST `/v2/account/activities` endpoint paginates across all
//! history; a point-in-time snapshot cannot determine a "recent fills" window
//! without additional context the adapter does not have.  Use `fetch_events`
//! (activity-polling path) for fill delivery.
//!
//! # No randomness, no wall-clock reads
//!
//! `captured_at_utc` for every `BrokerSnapshot` is **caller-injected** so
//! snapshot production is deterministic and testable without a live connection.

use chrono::{DateTime, Utc};
use mqk_execution::BrokerError;
use mqk_schemas::{BrokerAccount, BrokerOrder, BrokerPosition, BrokerSnapshot};

use crate::types::{AlpacaAccountRaw, AlpacaOpenOrderRaw, AlpacaPositionRaw};

// ---------------------------------------------------------------------------
// Normalization — pure, exported for isolated unit tests
// ---------------------------------------------------------------------------

/// Normalize an [`AlpacaAccountRaw`] wire response into the canonical
/// [`BrokerAccount`].
///
/// All fields are passed through verbatim as returned by Alpaca; no numeric
/// parsing is performed here so that callers observe the exact broker string.
pub fn normalize_account(raw: &AlpacaAccountRaw) -> BrokerAccount {
    BrokerAccount {
        equity: raw.equity.clone(),
        cash: raw.cash.clone(),
        currency: raw.currency.clone(),
        buying_power: raw.buying_power.clone(),
        daytrading_buying_power: raw.daytrading_buying_power.clone(),
    }
}

/// Quote assets Alpaca lists crypto pairs against. Longest first so a
/// `...USDT` suffix is never read as `...USD` plus a stray `T`.
const ALPACA_CRYPTO_QUOTE_ASSETS: [&str; 4] = ["USDT", "USDC", "USD", "BTC"];

/// Canonical (order-side) symbol for an Alpaca position.
///
/// Alpaca lists a crypto position under the consolidated asset symbol
/// (`BTCUSD`) while orders, fills and this system's canonical identity use the
/// pair form (`BTC/USD`). Only a position the broker itself labels `crypto`,
/// whose symbol has no `/` and ends in a known quote asset behind a nonempty
/// alphanumeric base, is rewritten; every other symbol is returned unchanged
/// so an unrecognized shape surfaces as reconcile drift instead of a guess.
pub fn canonical_alpaca_position_symbol(symbol: &str, asset_class: Option<&str>) -> String {
    let is_crypto = asset_class.is_some_and(|c| c.trim().eq_ignore_ascii_case("crypto"));
    let trimmed = symbol.trim();
    if !is_crypto || trimmed.contains('/') {
        return symbol.to_string();
    }
    for quote in ALPACA_CRYPTO_QUOTE_ASSETS {
        if let Some(base) = trimmed.strip_suffix(quote) {
            if !base.is_empty() && base.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return format!("{base}/{quote}");
            }
        }
    }
    symbol.to_string()
}

/// Normalize an [`AlpacaPositionRaw`] wire response into a canonical
/// [`BrokerPosition`].
///
/// `avg_price` maps from Alpaca's `avg_entry_price` field. The symbol is the
/// canonical order-side symbol ([`canonical_alpaca_position_symbol`]).
pub fn normalize_position(raw: &AlpacaPositionRaw) -> BrokerPosition {
    BrokerPosition {
        symbol: canonical_alpaca_position_symbol(&raw.symbol, raw.asset_class.as_deref()),
        qty: raw.qty.clone(),
        avg_price: raw.avg_entry_price.clone(),
    }
}

/// Normalize an [`AlpacaOpenOrderRaw`] wire response into a canonical
/// [`BrokerOrder`].
///
/// `created_at_utc` is parsed from the Alpaca ISO 8601 `created_at` field.
///
/// # Errors
///
/// Returns `Err(BrokerError::Transient)` if `created_at` is not a valid
/// RFC 3339 timestamp.  The entire snapshot fetch fails closed rather than
/// silently inserting a sentinel timestamp.
pub fn normalize_open_order(raw: &AlpacaOpenOrderRaw) -> Result<BrokerOrder, BrokerError> {
    let created_at_utc = chrono::DateTime::parse_from_rfc3339(&raw.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| BrokerError::Transient {
            detail: format!(
                "snapshot: cannot parse order created_at {:?}: {e}",
                raw.created_at
            ),
        })?;

    Ok(BrokerOrder {
        broker_order_id: raw.id.clone(),
        client_order_id: raw.client_order_id.clone(),
        symbol: raw.symbol.clone(),
        side: raw.side.clone(),
        r#type: raw.order_type.clone(),
        status: raw.status.clone(),
        qty: raw.qty.clone(),
        filled_qty: raw.filled_qty.clone(),
        limit_price: raw.limit_price.clone(),
        stop_price: raw.stop_price.clone(),
        created_at_utc,
    })
}

/// Assemble a [`BrokerSnapshot`] from pre-normalized components.
///
/// `fills` is always empty at AP-03.  Fill delivery is the responsibility of
/// the `fetch_events` activity-polling path, not the snapshot surface.
pub fn build_snapshot(
    captured_at_utc: DateTime<Utc>,
    account: BrokerAccount,
    positions: Vec<BrokerPosition>,
    orders: Vec<BrokerOrder>,
) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc,
        account,
        positions,
        orders,
        fills: vec![], // AP-03: fills not included; delivered by fetch_events
    }
}
