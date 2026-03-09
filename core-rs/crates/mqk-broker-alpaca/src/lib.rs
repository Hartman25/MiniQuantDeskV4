#![forbid(unsafe_code)]

//! Alpaca live broker adapter — A5 complete implementation.
//!
//! # Modules
//! - `types`     — raw Alpaca v2 wire shapes (REST + websocket).
//! - `normalize` — converts raw `AlpacaTradeUpdate` into canonical `BrokerEvent`.
//!
//! # `AlpacaBrokerAdapter`
//!
//! Implements `mqk_execution::BrokerAdapter` against the Alpaca v2 REST API using
//! `reqwest::blocking`.  All four methods are real HTTP calls:
//!
//! | Method          | Endpoint                               | Notes                              |
//! |-----------------|----------------------------------------|------------------------------------|
//! | `submit_order`  | `POST   /v2/orders`                    | AmbiguousSubmit on unknown timeout |
//! | `cancel_order`  | `DELETE /v2/orders/{broker_order_id}`  | 404/422 → Reject                   |
//! | `replace_order` | `GET+PATCH /v2/orders/{id}`            | Fetches filled_qty before PATCH    |
//! | `fetch_events`  | `GET /v2/account/activities`           | Polling; FILL/PARTIAL_FILL only    |
//!
//! # fetch_events polling limitation
//!
//! The websocket trade-update stream is the canonical source for all Alpaca order
//! lifecycle events (Ack, CancelAck, ReplaceAck, etc.).  The REST polling path
//! implemented here surfaces only `FILL` and `PARTIAL_FILL` activities.
//! Ack / cancel / replace events are NOT available via REST polling.
//! A websocket integration is required to deliver those in real-time.
//!
//! # No randomness, no wall-clock reads
//!
//! `AlpacaBrokerAdapter` itself introduces no timestamps or UUIDs.  All
//! identifiers used in canonical events come from the Alpaca response payload
//! and are normalised through `normalize_trade_update`.

pub mod normalize;
pub mod types;

use mqk_execution::{
    micros_to_price, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, Side,
};

use crate::normalize::normalize_trade_update;
use crate::types::{
    AlpacaOrder, AlpacaOrderActivity, AlpacaOrderFull, AlpacaReplaceBody, AlpacaReplaceResponse,
    AlpacaSubmitBody, AlpacaSubmitResponse, AlpacaTradeUpdate,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Connection configuration for the Alpaca REST API.
#[derive(Debug, Clone)]
pub struct AlpacaConfig {
    /// Base URL of the Alpaca API, e.g. `"https://api.alpaca.markets"` (live)
    /// or `"https://paper-api.alpaca.markets"` (paper trading).
    pub base_url: String,
    /// Alpaca API key ID (`APCA-API-KEY-ID` header).
    pub api_key_id: String,
    /// Alpaca API secret key (`APCA-API-SECRET-KEY` header).
    pub api_secret_key: String,
}

// ---------------------------------------------------------------------------
// AlpacaBrokerAdapter
// ---------------------------------------------------------------------------

/// Live Alpaca broker adapter.
///
/// Satisfies `mqk_execution::BrokerAdapter`.  Construct via
/// `AlpacaBrokerAdapter::new(cfg)` with explicit credentials.
pub struct AlpacaBrokerAdapter {
    cfg: AlpacaConfig,
    client: reqwest::blocking::Client,
}

impl std::fmt::Debug for AlpacaBrokerAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlpacaBrokerAdapter")
            .field("base_url", &self.cfg.base_url)
            .finish_non_exhaustive()
    }
}

impl AlpacaBrokerAdapter {
    /// Create a new adapter with the given configuration.
    ///
    /// A single `reqwest::blocking::Client` is shared across all calls made
    /// through this adapter instance.
    pub fn new(cfg: AlpacaConfig) -> Self {
        let client = reqwest::blocking::Client::new();
        Self { cfg, client }
    }

    /// Convenience constructor for Alpaca paper trading.
    pub fn paper(api_key_id: String, api_secret_key: String) -> Self {
        Self::new(AlpacaConfig {
            base_url: "https://paper-api.alpaca.markets".to_string(),
            api_key_id,
            api_secret_key,
        })
    }

    // -----------------------------------------------------------------------
    // Private HTTP helpers
    // -----------------------------------------------------------------------

    /// Perform an authenticated `GET` and deserialize the JSON response body.
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, BrokerError> {
        let url = format!("{}{}", self.cfg.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header("APCA-API-KEY-ID", &self.cfg.api_key_id)
            .header("APCA-API-SECRET-KEY", &self.cfg.api_secret_key)
            .send()
            .map_err(classify_transport_err)?;

        parse_success_json(resp)
    }

    /// Perform an authenticated `PATCH` with a JSON body; deserialize response.
    fn patch<B, T>(&self, path: &str, body: &B) -> Result<T, BrokerError>
    where
        B: serde::Serialize,
        T: serde::de::DeserializeOwned,
    {
        let url = format!("{}{}", self.cfg.base_url, path);
        let resp = self
            .client
            .patch(&url)
            .header("APCA-API-KEY-ID", &self.cfg.api_key_id)
            .header("APCA-API-SECRET-KEY", &self.cfg.api_secret_key)
            .json(body)
            .send()
            .map_err(classify_transport_err)?;

        parse_success_json(resp)
    }

    /// Perform an authenticated `DELETE`; return Ok(()) on success.
    fn delete(&self, path: &str) -> Result<(), BrokerError> {
        let url = format!("{}{}", self.cfg.base_url, path);
        let resp = self
            .client
            .delete(&url)
            .header("APCA-API-KEY-ID", &self.cfg.api_key_id)
            .header("APCA-API-SECRET-KEY", &self.cfg.api_secret_key)
            .send()
            .map_err(classify_transport_err)?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().unwrap_or_default();
            Err(classify_http_status(status, &body))
        }
    }

    /// Fetch a single order by its Alpaca broker order UUID.
    fn fetch_order(&self, broker_order_id: &str) -> Result<AlpacaOrderFull, BrokerError> {
        self.get(&format!("/v2/orders/{broker_order_id}"))
    }
}

// ---------------------------------------------------------------------------
// BrokerAdapter implementation
// ---------------------------------------------------------------------------

impl BrokerAdapter for AlpacaBrokerAdapter {
    /// Submit a new order to Alpaca.
    ///
    /// # Mapping
    /// - `req.order_id` → `client_order_id` (Alpaca echoes this on all events).
    /// - `req.limit_price` micros → decimal string at wire boundary only.
    /// - `req.quantity` is always positive; direction is carried by `side`.
    ///
    /// # Error classification
    /// - Connection refused → `Transport` (request never left the host).
    /// - Timeout / unknown network error → `AmbiguousSubmit` (order may be live).
    /// - HTTP 400/422 → `Reject`.
    /// - HTTP 401/403 → `AuthSession`.
    /// - HTTP 429 → `RateLimit`.
    /// - HTTP 5xx → `Transient`.
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BrokerError> {
        let body = build_submit_body(&req);

        let url = format!("{}/v2/orders", self.cfg.base_url);
        let http_resp = self
            .client
            .post(&url)
            .header("APCA-API-KEY-ID", &self.cfg.api_key_id)
            .header("APCA-API-SECRET-KEY", &self.cfg.api_secret_key)
            .json(&body)
            .send()
            .map_err(classify_transport_err_for_submit)?;

        let status = http_resp.status();
        if !status.is_success() {
            let resp_body = http_resp.text().unwrap_or_default();
            return Err(classify_http_status(status, &resp_body));
        }

        let alpaca: AlpacaSubmitResponse = http_resp.json().map_err(|e| {
            // We got a 2xx but couldn't parse the body.  The order may be live.
            BrokerError::AmbiguousSubmit {
                detail: format!("submit: response parse error: {e}"),
            }
        })?;

        Ok(BrokerSubmitResponse {
            broker_order_id: alpaca.id,
            submitted_at: alpaca
                .created_at
                .as_deref()
                .and_then(parse_iso_to_epoch_ms)
                .unwrap_or(0),
            status: "acknowledged".to_string(),
        })
    }

    /// Cancel an in-flight order by its authoritative Alpaca broker order UUID.
    ///
    /// # Status mapping
    /// - HTTP 204 No Content → success.
    /// - HTTP 404 → `Reject` (order not found; may have already been filled or expired).
    /// - HTTP 422 → `Reject` (unprocessable; order already in a terminal state).
    /// - Other errors → standard classification.
    fn cancel_order(
        &self,
        broker_order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, BrokerError> {
        self.delete(&format!("/v2/orders/{broker_order_id}"))?;

        Ok(BrokerCancelResponse {
            broker_order_id: broker_order_id.to_string(),
            // Alpaca DELETE /v2/orders returns 204 No Content — no timestamp in body.
            cancelled_at: 0,
            status: "cancelled".to_string(),
        })
    }

    /// Replace an in-flight order with a new quantity and/or price.
    ///
    /// # Alpaca total-quantity semantics
    ///
    /// Alpaca's PATCH endpoint interprets `qty` as the **new total** quantity
    /// (filled + open leaves), not the new open leaves alone.  This adapter
    /// therefore:
    ///
    /// 1. Calls `GET /v2/orders/{broker_order_id}` to read the current
    ///    `filled_qty` from Alpaca.
    /// 2. Computes `new_total_qty = filled_qty + req.quantity` (where
    ///    `req.quantity` is the canonical new-open-leaves quantity).
    /// 3. Sends the PATCH with the computed total.
    ///
    /// If the `GET` fails (transport error, broker error), or if `filled_qty`
    /// cannot be parsed from the response, the adapter **fails closed** and
    /// returns the error rather than guessing.
    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, BrokerError> {
        // Step 1: fetch current order state to obtain filled_qty.
        let order: AlpacaOrderFull = self.fetch_order(&req.broker_order_id)?;

        // Parse filled_qty — fail closed if it is malformed.
        let filled_qty =
            parse_broker_qty(&order.filled_qty).map_err(|raw| BrokerError::Transient {
                detail: format!("replace: non-parseable filled_qty from broker: {raw:?}"),
            })?;

        // Step 2: build replace body with Alpaca total-qty semantics.
        let body = build_replace_body(
            req.quantity,
            filled_qty,
            req.limit_price,
            &req.time_in_force,
        );

        // Step 3: send PATCH.
        let _: AlpacaReplaceResponse =
            self.patch(&format!("/v2/orders/{}", req.broker_order_id), &body)?;

        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            // Alpaca PATCH does not guarantee a timestamp in the response.
            replaced_at: 0,
            status: "replace_requested".to_string(),
        })
    }

    /// Poll Alpaca for recent order fill events.
    ///
    /// # Polling path
    ///
    /// Uses `GET /v2/account/activities?direction=asc&page_size=50[&after={cursor}]`.
    /// Only `FILL` and `PARTIAL_FILL` activity types are converted to canonical
    /// `BrokerEvent`s.  Other activity types (dividends, adjustments) are silently
    /// skipped.
    ///
    /// For each fill activity, the adapter calls `GET /v2/orders/{order_id}` to
    /// obtain the `client_order_id` required for `internal_order_id` mapping.
    /// All events are passed through `normalize_trade_update`.
    ///
    /// # Cursor
    ///
    /// The cursor is the `id` of the last activity returned by a prior call
    /// (e.g. `"20240615093001000::uuid"`).  Pass `None` to start from the
    /// beginning.  The returned cursor is the `id` of the last activity in this
    /// page, or `None` if no activities were returned.
    ///
    /// # Limitation
    ///
    /// Ack, CancelAck, and ReplaceAck events are not available via REST polling.
    /// A websocket integration against Alpaca's trade-update stream is required
    /// to deliver those lifecycle events.
    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        // Build the polling URL.
        let mut path = "/v2/account/activities?direction=asc&page_size=50".to_string();
        if let Some(c) = cursor {
            path.push_str("&after=");
            path.push_str(c);
        }

        let activities: Vec<AlpacaOrderActivity> = self.get(&path)?;

        // Compute new cursor before consuming the vec.
        let new_cursor = activities.last().map(|a| a.id.clone());

        let mut events = Vec::new();
        for activity in &activities {
            // Skip non-fill activity types silently.
            if !matches!(activity.activity_type.as_str(), "FILL" | "PARTIAL_FILL") {
                continue;
            }

            // Look up the order to get client_order_id.
            let order = self.fetch_order(&activity.order_id)?;

            // Build AlpacaTradeUpdate from activity + order, then normalize.
            let trade_update =
                activity_to_trade_update(activity, &order).map_err(|e| BrokerError::Transient {
                    detail: format!("fetch_events: activity mapping error: {e}"),
                })?;

            let event =
                normalize_trade_update(&trade_update).map_err(|e| BrokerError::Transient {
                    detail: format!("fetch_events: normalize error: {e}"),
                })?;

            events.push(event);
        }

        Ok((events, new_cursor))
    }
}

// ---------------------------------------------------------------------------
// Public pure functions (exported for testing)
// ---------------------------------------------------------------------------

/// Build an `AlpacaSubmitBody` from a canonical `BrokerSubmitRequest`.
///
/// - `side` is mapped from `Side::Buy`/`Sell` to `"buy"`/`"sell"`.
/// - `quantity` is always positive (direction is in `side`).
/// - `limit_price` micros are converted to a decimal string **only here**,
///   at the wire boundary.
/// - `client_order_id` is set to `req.order_id` so Alpaca echoes it back
///   on every lifecycle event, enabling `internal_order_id` mapping.
pub fn build_submit_body(req: &BrokerSubmitRequest) -> AlpacaSubmitBody {
    let side = side_to_str(&req.side);
    let limit_price = req.limit_price.map(micros_to_price_str);
    AlpacaSubmitBody {
        symbol: req.symbol.clone(),
        qty: req.quantity.to_string(),
        side: side.to_string(),
        order_type: req.order_type.clone(),
        time_in_force: req.time_in_force.clone(),
        limit_price,
        client_order_id: req.order_id.clone(),
    }
}

/// Build an `AlpacaReplaceBody` applying Alpaca total-quantity semantics.
///
/// Alpaca PATCH interprets `qty` as the **new total** (filled + open leaves).
/// The canonical `BrokerReplaceRequest.quantity` carries the new open-leaves
/// count.  This function computes `new_total = filled_qty + new_leaves_qty`.
pub fn build_replace_body(
    new_leaves_qty: i32,
    filled_qty: i64,
    limit_price: Option<i64>,
    time_in_force: &str,
) -> AlpacaReplaceBody {
    let new_total_qty = filled_qty + new_leaves_qty as i64;
    AlpacaReplaceBody {
        qty: new_total_qty.to_string(),
        limit_price: limit_price.map(micros_to_price_str),
        time_in_force: time_in_force.to_string(),
    }
}

/// Convert an `AlpacaOrderActivity` (REST polling) into an `AlpacaTradeUpdate`
/// (normalizer input), given the full order state from a parallel order lookup.
///
/// Only `"FILL"` and `"PARTIAL_FILL"` activity types are supported.
///
/// # Errors
///
/// Returns `Err(String)` if `activity.activity_type` is not a recognised fill type.
pub fn activity_to_trade_update(
    activity: &AlpacaOrderActivity,
    order: &AlpacaOrderFull,
) -> Result<AlpacaTradeUpdate, String> {
    // Map Alpaca uppercase activity_type to normalizer event string.
    let event_type = match activity.activity_type.as_str() {
        "FILL" => "fill",
        "PARTIAL_FILL" => "partial_fill",
        other => {
            return Err(format!(
                "activity_to_trade_update: unsupported activity_type: {other:?}"
            ))
        }
    };

    let alpaca_order = AlpacaOrder {
        id: order.id.clone(),
        client_order_id: order.client_order_id.clone(),
        symbol: order.symbol.clone(),
        side: order.side.clone(),
        qty: order.qty.clone(),
        filled_qty: order.filled_qty.clone(),
    };

    Ok(AlpacaTradeUpdate {
        event: event_type.to_string(),
        timestamp: activity.transaction_time.clone(),
        order: alpaca_order,
        price: activity.price.clone(),
        qty: activity.qty.clone(),
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Convert a canonical `Side` to the Alpaca wire string.
fn side_to_str(side: &Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

/// Convert integer micros to a decimal price string for the broker wire.
///
/// **Only call at the wire boundary** — this is the sole site in this crate
/// that crosses the i64-micros / f64-decimal boundary for prices.
///
/// Uses 2 decimal places, which covers all standard US equity prices.
/// Sub-cent precision is handled by the 6-decimal scale if needed by
/// callers that have set fractional micros, but US equities trade in cents.
pub fn micros_to_price_str(micros: i64) -> String {
    // micros_to_price returns f64; format to 2 decimal places for the wire.
    format!("{:.2}", micros_to_price(micros))
}

/// Parse a broker decimal quantity string (e.g. `"100.000000"`) to `i64`.
///
/// Returns `Err(raw)` if the string is not a finite non-negative number.
fn parse_broker_qty(raw: &str) -> Result<i64, &str> {
    let v: f64 = raw.parse().map_err(|_| raw)?;
    if !v.is_finite() || v < 0.0 {
        return Err(raw);
    }
    Ok(v.round() as i64)
}

/// Parse an ISO 8601 timestamp to Unix epoch milliseconds.
fn parse_iso_to_epoch_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

/// Read the HTTP response: return parsed JSON on success, or classify the
/// error on failure.
fn parse_success_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::blocking::Response,
) -> Result<T, BrokerError> {
    let status = resp.status();
    if status.is_success() {
        resp.json::<T>().map_err(|e| BrokerError::Transient {
            detail: format!("response parse error: {e}"),
        })
    } else {
        let body = resp.text().unwrap_or_default();
        Err(classify_http_status(status, &body))
    }
}

/// Map a `reqwest::Error` to `BrokerError` for **submit** calls.
///
/// For submit, any error that is not a clean connection refusal is treated
/// as `AmbiguousSubmit` because the order may have reached the broker.
fn classify_transport_err_for_submit(err: reqwest::Error) -> BrokerError {
    if err.is_connect() {
        // Connection refused before the request was sent — safe to retry.
        BrokerError::Transport {
            detail: err.to_string(),
        }
    } else {
        // Timeout, builder error, or mid-flight failure — order may be live.
        BrokerError::AmbiguousSubmit {
            detail: err.to_string(),
        }
    }
}

/// Map a `reqwest::Error` to `BrokerError` for all non-submit calls.
fn classify_transport_err(err: reqwest::Error) -> BrokerError {
    if err.is_connect() {
        BrokerError::Transport {
            detail: err.to_string(),
        }
    } else if err.is_timeout() {
        BrokerError::Transient {
            detail: format!("timeout: {err}"),
        }
    } else {
        BrokerError::Transient {
            detail: err.to_string(),
        }
    }
}

/// Map an HTTP status code + response body to a typed `BrokerError`.
fn classify_http_status(status: reqwest::StatusCode, body: &str) -> BrokerError {
    match status.as_u16() {
        401 | 403 => BrokerError::AuthSession {
            detail: body.to_string(),
        },
        400 | 422 => BrokerError::Reject {
            code: status.as_str().to_string(),
            detail: body.to_string(),
        },
        404 => BrokerError::Reject {
            code: "404".to_string(),
            detail: format!("not found: {body}"),
        },
        429 => BrokerError::RateLimit {
            retry_after_ms: None,
            detail: body.to_string(),
        },
        c if c >= 500 => BrokerError::Transient {
            detail: format!("HTTP {c}: {body}"),
        },
        c => BrokerError::Transient {
            detail: format!("HTTP {c}: {body}"),
        },
    }
}
