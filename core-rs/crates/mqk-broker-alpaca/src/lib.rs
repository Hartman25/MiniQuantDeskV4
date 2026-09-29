#![forbid(unsafe_code)]
//! Alpaca live broker adapter - A5 complete implementation.
//!
//! # Modules
//! - `types`     - raw Alpaca v2 wire shapes (REST + websocket).
//! - `normalize` - converts raw `AlpacaTradeUpdate` into canonical `BrokerEvent`.
//!
//! Legacy non-functional gateway scaffolding has been removed from the
//! production adapter surface; this crate exports only live adapter paths.
//!
//! # Crate scope: REST order lifecycle only, not WS transport
//!
//! This crate owns Alpaca's REST order-lifecycle surface (submit/cancel/
//! replace/poll) and inbound trade-update normalization. Alpaca WebSocket
//! transport and gap-recovery live outside this crate, in `mqk-daemon`
//! (`state/alpaca_ws_transport.rs`, `state/ws_gap_recovery.rs`) — despite
//! this crate's name suggesting it owns the full broker surface. Do not
//! assume WS logic is colocated with the REST/normalize logic here.
//!
//! # `AlpacaBrokerAdapter`
//!
//! Implements `mqk_execution::BrokerAdapter` against the Alpaca v2 REST API using
//! `reqwest` (async client).  All four methods are real HTTP calls:
//!
//! | Method          | Endpoint                               | Notes                              |
//! |-----------------|----------------------------------------|------------------------------------|
//! | `submit_order`  | `POST   /v2/orders`                    | AmbiguousSubmit on unknown timeout |
//! | `cancel_order`  | `DELETE /v2/orders/{broker_order_id}`  | 404/422 → Reject                   |
//! | `replace_order` | `GET+PATCH /v2/orders/{id}`            | Fetches filled_qty before PATCH    |
//! | `fetch_events`  | `GET /v2/account/activities/FILL`      | Polling; maps fill lifecycle only   |
//!
//! # Inbound lifecycle coverage
//!
//! **Normalization boundary (fully proven):** `normalize_trade_update` handles
//! all 8 canonical lifecycle variants - Ack, PartialFill, Fill, CancelAck,
//! CancelReject, ReplaceAck, ReplaceReject, Reject - as proven by contract
//! tests (C1-C10), inbound lifecycle tests (IL-1-IL-11), and canonical
//! event-mapping tests (BRK-03R/04R/05R/06R).
//!
//! **Websocket inbound lane (BRK-01R, complete):** `parse_ws_message` +
//! `build_inbound_batch_from_ws_update` deliver the full lifecycle for all
//! 11 Alpaca event strings → 8 canonical `BrokerEvent` variants.  All event
//! types (Ack, CancelAck, CancelReject, ReplaceAck, ReplaceReject, Reject,
//! PartialFill, Fill) are proven to flow through the WS ingest path.
//!
//! **REST activity polling boundary:** `fetch_events` polls the type-specific
//! `GET /v2/account/activities/FILL` endpoint so the REST payload contains only
//! fill-class trade activities.  Alpaca's unfiltered account-activities feed can
//! include non-order records (for example `JNLC` and `FEE`) that do not match
//! the trade-activity schema this adapter normalises.  All non-fill lifecycle
//! events remain authoritative via the WS path.
//!
//! # No randomness, no wall-clock reads
//!
//! `AlpacaBrokerAdapter` itself introduces no timestamps or UUIDs.  All
//! identifiers used in canonical events come from the Alpaca response payload
//! and are normalised through `normalize_trade_update`.
pub mod fee_attribution;
pub mod fill_authority;
pub mod inbound;
pub mod mleg;
pub mod normalize;
pub mod option_lifecycle_normalize;
pub mod snapshot;
pub mod types;
use crate::mleg::{
    build_mleg_submit_body, normalize_mleg_submit_response, MlegRefusal, SubmittedVerticalSpread,
    VerifiedVerticalSpreadSubmission,
};
use crate::normalize::normalize_trade_update;
use crate::types::{
    AlpacaAccountIdentityRaw, AlpacaAccountRaw, AlpacaAssetRaw, AlpacaFeeActivity,
    AlpacaFetchCursor, AlpacaMlegSubmitResponse, AlpacaOpenOrderRaw, AlpacaOrder,
    AlpacaOrderActivity, AlpacaOrderFull, AlpacaPositionRaw, AlpacaReplaceBody,
    AlpacaReplaceResponse, AlpacaSubmitBody, AlpacaSubmitResponse, AlpacaTradeUpdate,
    AlpacaTradeUpdatesResume,
};
pub use fee_attribution::{normalize_fee_activity, FeeAttributionRecord, FeeNormalizeError};
pub use inbound::{
    build_inbound_batch_from_ws_update, mark_gap_detected, parse_ws_message, AlpacaWsMessage,
    InboundBatch, WsParseError,
};
use mqk_execution::{
    micros_to_price, AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, QtyMicros, Side,
};
use mqk_schemas::BrokerSnapshot;
pub use snapshot::{
    build_snapshot, canonical_alpaca_position_symbol, normalize_account, normalize_open_order,
    normalize_position,
};
/// Number of FILL activities requested per page from Alpaca REST.
///
/// Alpaca's API maximum is 100; 50 is a conservative default that leaves room
/// for any per-account rate-limit budgets.  `fetch_events` loops until the
/// response contains fewer than this many items, recovering all available
/// activities in one call.
pub const FILL_ACTIVITIES_PAGE_SIZE: usize = 50;
/// Maximum pages [`AlpacaBrokerAdapter::fetch_fill_activities_for_order`]
/// will scan before refusing to return a possibly-incomplete result set.
/// 40 pages * 50/page = 2000 activities — a generous operational bound for a
/// single-order repair lookup; see that method's doc comment.
pub const FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES: usize = 40;
// ---------------------------------------------------------------------------
// CUTOVER-1C Phase 2: Alpaca crypto order-quantity validation
// ---------------------------------------------------------------------------
/// The only crypto trading pair this adapter validates order quantities for.
///
/// Alpaca supports many crypto pairs; this adapter only claims correctness
/// for BTC/USD (the mission-scoped pair). A Crypto-class order for any other
/// symbol fails closed rather than silently applying BTC/USD's min-qty and
/// increment constants to a pair they were never verified against — there is
/// no pair registry yet (`mqk_execution::asset_risk_policy::crypto_policy`
/// documents this gap explicitly).
pub const ALPACA_CRYPTO_SUPPORTED_SYMBOL: &str = "BTC/USD";
/// Alpaca's documented BTC/USD minimum order size, in raw `QtyMicros` units
/// (0.0001 BTC * 1_000_000 micros/unit = 100).
pub const ALPACA_BTCUSD_MIN_ORDER_QTY_RAW: i64 = 100;
/// Alpaca's documented BTC/USD minimum trade increment, in raw `QtyMicros`
/// units. Order quantity must be an exact whole multiple of this value.
pub const ALPACA_BTCUSD_QTY_INCREMENT_RAW: i64 = 100;
/// Validate a Crypto-class order quantity against Alpaca's documented
/// per-pair minimum order size and trade increment before any HTTP call is
/// made. Fails closed (`BrokerError::Reject`) rather than letting the broker
/// reject it after the fact, so a malformed crypto order quantity never
/// consumes an outbox submit attempt.
///
/// Only `symbol == "BTC/USD"` is validated; any other Crypto symbol is
/// refused as unsupported (see [`ALPACA_CRYPTO_SUPPORTED_SYMBOL`]).
pub fn validate_alpaca_crypto_order_qty(symbol: &str, qty: QtyMicros) -> Result<(), BrokerError> {
    if symbol != ALPACA_CRYPTO_SUPPORTED_SYMBOL {
        return Err(BrokerError::Reject {
            code: "crypto_pair_unsupported".to_string(),
            detail: format!(
                "validate_alpaca_crypto_order_qty: only {ALPACA_CRYPTO_SUPPORTED_SYMBOL} is \
                 supported by this adapter; got symbol={symbol:?}"
            ),
        });
    }
    let raw = qty.raw();
    if raw < ALPACA_BTCUSD_MIN_ORDER_QTY_RAW {
        return Err(BrokerError::Reject {
            code: "crypto_qty_below_minimum".to_string(),
            detail: format!(
                "validate_alpaca_crypto_order_qty: qty={qty} is below BTC/USD minimum order \
                 size of 0.0001 BTC"
            ),
        });
    }
    if raw % ALPACA_BTCUSD_QTY_INCREMENT_RAW != 0 {
        return Err(BrokerError::Reject {
            code: "crypto_qty_not_multiple_of_increment".to_string(),
            detail: format!(
                "validate_alpaca_crypto_order_qty: qty={qty} is not an exact multiple of \
                 BTC/USD's minimum trade increment of 0.0001 BTC"
            ),
        });
    }
    Ok(())
}
/// Time-in-force values Alpaca accepts for crypto orders (docs: "the
/// supported `time_in_force` values are `gtc`, and `ioc`").
pub const ALPACA_CRYPTO_SUPPORTED_TIME_IN_FORCE: [&str; 2] = ["gtc", "ioc"];
/// Refuse a crypto order (or replace) whose time-in-force Alpaca does not
/// support before any HTTP call, instead of consuming a submit attempt on a
/// guaranteed broker rejection. There is no equity-style `day` for crypto.
pub fn validate_alpaca_crypto_order_tif(time_in_force: &str) -> Result<(), BrokerError> {
    let tif = time_in_force.trim().to_ascii_lowercase();
    if ALPACA_CRYPTO_SUPPORTED_TIME_IN_FORCE.contains(&tif.as_str()) {
        return Ok(());
    }
    Err(BrokerError::Reject {
        code: "crypto_time_in_force_unsupported".to_string(),
        detail: format!(
            "validate_alpaca_crypto_order_tif: crypto orders support only \
             {ALPACA_CRYPTO_SUPPORTED_TIME_IN_FORCE:?}; got time_in_force={time_in_force:?}"
        ),
    })
}
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
    /// D2/B4 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01): explicit,
    /// default-off Crypto capability. Never `true` in any checked-in
    /// default constructor (`paper()`, `live()`, `new_for_test()`) -- a
    /// caller must opt in explicitly via [`AlpacaConfig::with_crypto_capability_enabled`].
    ///
    /// This flag is necessary but never sufficient: [`AlpacaBrokerAdapter::
    /// supports_asset_class`] additionally requires [`AlpacaConfig::
    /// targets_paper_api`] to be `true` before ever advertising Crypto, so a
    /// Live-targeting config can never advertise Crypto regardless of this
    /// field's value -- a structural guarantee, not a convention.
    pub crypto_capability_enabled: bool,
    /// D5 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
    /// explicit, default-off multi-leg (mleg) vertical-spread options
    /// order capability. Never `true` in any checked-in default
    /// constructor (`paper()`, `live()`, `new_for_test()`) -- a caller
    /// must opt in explicitly via
    /// [`AlpacaBrokerAdapter::with_options_mleg_capability_enabled`].
    /// Gates [`AlpacaBrokerAdapter::submit_vertical_spread`] only; it does
    /// not affect `supports_asset_class` or single-leg equity/crypto
    /// order submission at all.
    pub options_mleg_capability_enabled: bool,
}

impl AlpacaConfig {
    /// `true` iff `base_url` targets Alpaca's PAPER API host. The sole
    /// authority `supports_asset_class` consults (together with
    /// `crypto_capability_enabled`) to decide Crypto capability -- never
    /// inferred from deployment-mode labels or any other config field.
    fn targets_paper_api(&self) -> bool {
        self.base_url.contains("paper-api.alpaca.markets")
    }
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
    client: reqwest::Client,
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
    /// A single `reqwest::Client` is shared across all calls made through this
    /// adapter instance.  `reqwest::Client` is cheaply cloneable (Arc-backed).
    /// Construct a new adapter, trimming whitespace and control characters
    /// (e.g. CRLF from Windows `.env` files) from all three config fields.
    ///
    /// HTTP header values must not contain control characters; an untrimmed
    /// `\r` or `\n` in a credential string causes reqwest to fail with an
    /// opaque "builder error" before any TCP connection is attempted.
    /// Trimming here is the single canonical seam — `paper()`, `live()`, and
    /// all `build_*_from_env` helpers all go through this constructor.
    pub fn new(cfg: AlpacaConfig) -> Self {
        let client = reqwest::Client::new();
        Self {
            cfg: AlpacaConfig {
                base_url: cfg.base_url.trim().to_owned(),
                api_key_id: cfg.api_key_id.trim().to_owned(),
                api_secret_key: cfg.api_secret_key.trim().to_owned(),
                crypto_capability_enabled: cfg.crypto_capability_enabled,
                options_mleg_capability_enabled: cfg.options_mleg_capability_enabled,
            },
            client,
        }
    }

    /// D2/B4: explicit opt-in for the Crypto capability flag, callable on
    /// any already-constructed adapter. Never wired to a checked-in default
    /// -- an operator/config layer above this crate calls it explicitly.
    /// Actual capability still requires [`AlpacaConfig::targets_paper_api`]
    /// (see `supports_asset_class`); calling this on a `live()` adapter sets
    /// the flag but has no capability effect.
    pub fn with_crypto_capability_enabled(mut self, enabled: bool) -> Self {
        self.cfg.crypto_capability_enabled = enabled;
        self
    }

    /// The authenticated Alpaca trading account's own provider account id
    /// (`id` of `GET /v2/account`), never the API key id: a credential
    /// rotation on the same account must not manufacture a new economic
    /// account. A missing/blank/non-UUID-shaped `id` fails closed -- no
    /// identity is ever derived from the credential.
    pub fn fetch_provider_account_id(&self) -> Result<String, BrokerError> {
        let raw: AlpacaAccountIdentityRaw = self.get("/v2/account")?;
        let id = raw.id.as_deref().map(str::trim).unwrap_or_default();
        if id.is_empty()
            || id.len() > 128
            || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(BrokerError::Reject {
                code: "account_id_unavailable".to_string(),
                detail: "GET /v2/account did not return a usable account id; refusing to derive                          an account identity from the API credential"
                    .to_string(),
            });
        }
        Ok(id.to_ascii_lowercase())
    }

    /// D2/B4: the Crypto capability flag as configured (not the same as
    /// whether Crypto is actually advertised -- see `supports_asset_class`,
    /// which additionally requires a Paper-targeting `base_url`). Exposed
    /// for status/provenance surfaces so the operator-visible capability
    /// choice is never silently inferred from deployment-mode labels alone.
    pub fn crypto_capability_enabled(&self) -> bool {
        self.cfg.crypto_capability_enabled
    }

    /// D5: explicit opt-in for the multi-leg vertical-spread options
    /// capability, callable on any already-constructed adapter. Never
    /// wired to a checked-in default -- an operator/config layer above
    /// this crate calls it explicitly.
    pub fn with_options_mleg_capability_enabled(mut self, enabled: bool) -> Self {
        self.cfg.options_mleg_capability_enabled = enabled;
        self
    }

    /// D5: the multi-leg vertical-spread capability flag as configured.
    /// `submit_vertical_spread` refuses fail-closed whenever this is
    /// `false`.
    pub fn options_mleg_capability_enabled(&self) -> bool {
        self.cfg.options_mleg_capability_enabled
    }

    /// Test constructor: injects a mock base URL so `Retry-After`-threading
    /// tests can point the adapter at an in-process mock server.
    #[cfg(test)]
    fn new_for_test(base_url: String) -> Self {
        Self::new(AlpacaConfig {
            base_url,
            api_key_id: "test-key".to_string(),
            api_secret_key: "test-secret".to_string(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
    }
    /// Convenience constructor for Alpaca paper trading.
    ///
    /// Targets `https://paper-api.alpaca.markets`.  Use for `(Paper, Alpaca)`
    /// deployment mode only.  Do NOT use for live-shadow or live-capital.
    ///
    /// D2/B4: Crypto capability defaults to `false` here -- call
    /// `.with_crypto_capability_enabled(true)` on the result to opt in
    /// explicitly. This is the ONLY constructor for which that opt-in can
    /// ever have an effect (see `targets_paper_api`).
    pub fn paper(api_key_id: String, api_secret_key: String) -> Self {
        Self::new(AlpacaConfig {
            base_url: "https://paper-api.alpaca.markets".to_string(),
            api_key_id,
            api_secret_key,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
    }

    /// Convenience constructor for Alpaca live (real-market) connectivity.
    ///
    /// Targets `https://api.alpaca.markets`.  Use for `(LiveShadow, Alpaca)`
    /// deployment mode.  Do NOT use for paper-only deployments.
    ///
    /// D2/B4: Crypto capability is always `false` and structurally cannot
    /// become effective for this adapter -- `supports_asset_class` also
    /// requires `targets_paper_api()`, which a live `base_url` never
    /// satisfies.
    pub fn live(api_key_id: String, api_secret_key: String) -> Self {
        Self::new(AlpacaConfig {
            base_url: "https://api.alpaca.markets".to_string(),
            api_key_id,
            api_secret_key,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
    }
    // -----------------------------------------------------------------------
    // Private HTTP helpers
    // -----------------------------------------------------------------------

    /// Drive an async future to completion from a sync caller.
    ///
    /// When called from within a Tokio multi-thread async task (the normal
    /// production path), `block_in_place` suspends the current task and
    /// drives the future on the current OS thread via the existing runtime's
    /// `Handle::block_on`.  No embedded runtime is created; `reqwest`'s
    /// connection pool shares the outer runtime's I/O reactor.
    ///
    /// When called with no Tokio runtime present (plain `#[test]` contexts
    /// or CLI tools), a minimal `current_thread` runtime is created for the
    /// duration of the call and then dropped.  This matches the behavior of
    /// `reqwest::blocking` in the same context, with no extra overhead.
    ///
    /// Nested calls from within a daemon-side `block_in_place` wrapper are
    /// safe: Tokio detects the already-blocking thread state and executes the
    /// inner closure directly without a redundant scheduler transition.
    fn run_async<F>(f: F) -> F::Output
    where
        F: std::future::Future,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(f)),
            Err(_) => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("broker: failed to build tokio runtime for sync HTTP call")
                .block_on(f),
        }
    }

    /// Perform an authenticated `GET` and deserialize the JSON response body.
    ///
    /// A single attempt, exactly like every other call site in this adapter
    /// -- no in-adapter retry policy. A 429/5xx surfaces to the caller with
    /// any parsed `Retry-After` guidance threaded onto `BrokerError::
    /// RateLimit` (see [`parse_success_response`]) for the orchestrator's
    /// existing outbox-driven redispatch authority to act on.
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, BrokerError> {
        let url = format!("{}{}", self.cfg.base_url, path);
        let client = self.client.clone();
        let key_id = self.cfg.api_key_id.clone();
        let secret_key = self.cfg.api_secret_key.clone();
        Self::run_async(async move {
            let resp = client
                .get(&url)
                .header("APCA-API-KEY-ID", &key_id)
                .header("APCA-API-SECRET-KEY", &secret_key)
                .send()
                .await
                .map_err(classify_transport_err)?;
            parse_success_response(resp).await
        })
    }
    /// Perform an authenticated `PATCH` with a JSON body; deserialize response.
    fn patch<B, T>(&self, path: &str, body: &B) -> Result<T, BrokerError>
    where
        B: serde::Serialize,
        T: serde::de::DeserializeOwned,
    {
        let url = format!("{}{}", self.cfg.base_url, path);
        let client = self.client.clone();
        let key_id = self.cfg.api_key_id.clone();
        let secret_key = self.cfg.api_secret_key.clone();
        let body_bytes = serde_json::to_vec(body).map_err(|e| BrokerError::Transient {
            detail: format!("patch: body serialize error: {e}"),
        })?;
        Self::run_async(async move {
            let resp = client
                .patch(&url)
                .header("APCA-API-KEY-ID", &key_id)
                .header("APCA-API-SECRET-KEY", &secret_key)
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .map_err(classify_transport_err)?;
            parse_success_response(resp).await
        })
    }
    /// Perform an authenticated `DELETE`; return Ok(()) on success.
    ///
    /// Not retried in-adapter: a 429/5xx here surfaces to the orchestrator's
    /// existing outbox-driven redispatch authority (`non_delivery_proven`)
    /// rather than being retried blindly against a mutating endpoint.
    fn delete(&self, path: &str) -> Result<(), BrokerError> {
        let url = format!("{}{}", self.cfg.base_url, path);
        let client = self.client.clone();
        let key_id = self.cfg.api_key_id.clone();
        let secret_key = self.cfg.api_secret_key.clone();
        Self::run_async(async move {
            let resp = client
                .delete(&url)
                .header("APCA-API-KEY-ID", &key_id)
                .header("APCA-API-SECRET-KEY", &secret_key)
                .send()
                .await
                .map_err(classify_transport_err)?;
            let status = resp.status();
            if status.is_success() {
                Ok(())
            } else {
                let retry_after_ms = parse_retry_after_ms(
                    resp.headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()),
                );
                let body = resp.text().await.unwrap_or_default();
                Err(classify_http_status(status, &body, retry_after_ms))
            }
        })
    }
    /// Fetch a single order by its Alpaca broker order UUID.
    fn fetch_order(&self, broker_order_id: &str) -> Result<AlpacaOrderFull, BrokerError> {
        self.get(&format!("/v2/orders/{broker_order_id}"))
    }

    // -----------------------------------------------------------------------
    // BROKER-FILL-REST-PRODUCTION-WIRING-01: per-order fill activity fetch
    // -----------------------------------------------------------------------
    /// Fetch all FILL-class account activities for one Alpaca broker order UUID.
    ///
    /// PAPER-SOAK-ALPACA-FILL-ECONOMIC-AUTHORITY-CLOSURE-01: Alpaca's primary
    /// Account Activities documentation does not document `order_id` as a
    /// supported query parameter for `GET /v2/account/activities/FILL` — a
    /// prior implementation sent `?order_id={broker_order_id}` and trusted
    /// the broker to filter server-side. That is not an acceptable safety
    /// assumption for a repair path that may mutate durable accounting
    /// truth, so this method no longer sends it. Instead it paginates
    /// through the account-wide FILL activity feed using only documented
    /// parameters (`direction`, `page_size`, `page_token`) and applies an
    /// exact `activity.order_id == broker_order_id` filter locally.
    ///
    /// Pagination walks in `direction=desc` (most-recent-first — the target
    /// order's activity is expected to be recent) to exhaustion (a page
    /// shorter than [`FILL_ACTIVITIES_PAGE_SIZE`]) or up to
    /// [`FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES`], whichever comes first. If the
    /// page budget is exhausted before reaching the end of the feed, this
    /// returns `Err` rather than a possibly-incomplete match set — a false
    /// "exactly one activity" conclusion merely because a matching activity
    /// sat outside the searched window would be unsafe for a route that may
    /// mutate durable accounting truth.
    ///
    /// Does not affect order submission, cancellation, replacement, or the
    /// `fetch_events` inbound lane.  Read-only.
    pub fn fetch_fill_activities_for_order(
        &self,
        broker_order_id: &str,
    ) -> Result<Vec<AlpacaOrderActivity>, BrokerError> {
        let mut current_page_token: Option<String> = None;
        let mut matched: Vec<AlpacaOrderActivity> = Vec::new();
        let mut pages: usize = 0;

        loop {
            let mut path = format!(
                "/v2/account/activities/FILL?direction=desc&page_size={FILL_ACTIVITIES_PAGE_SIZE}"
            );
            if let Some(token) = current_page_token.as_deref() {
                path.push_str("&page_token=");
                path.push_str(token);
            }

            let activities: Vec<AlpacaOrderActivity> = self.get(&path)?;
            let page_len = activities.len();
            pages += 1;
            let prev_page_token = current_page_token.clone();

            if let Some(last) = activities.last() {
                current_page_token = Some(last.id.clone());
            }

            if page_len == FILL_ACTIVITIES_PAGE_SIZE && current_page_token == prev_page_token {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_fill_activities_for_order: pagination made no progress at \
                         page_token={prev_page_token:?}; refusing to loop"
                    ),
                });
            }

            matched.extend(
                activities
                    .into_iter()
                    .filter(|a| a.order_id == broker_order_id),
            );

            if page_len < FILL_ACTIVITIES_PAGE_SIZE {
                return Ok(matched);
            }

            if pages >= FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_fill_activities_for_order: exhausted {FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES} \
                         pages ({} activities scanned) without reaching the end of the account \
                         activity feed for broker_order_id='{broker_order_id}'; refusing to return a \
                         possibly-incomplete result set — manual reconcile required",
                        FILL_ACTIVITIES_FOR_ORDER_MAX_PAGES * FILL_ACTIVITIES_PAGE_SIZE
                    ),
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // BRK-GAP-REST-RECOVERY-01: account-wide fill fetch since cursor
    // -----------------------------------------------------------------------
    /// Fetch all FILL-class account activities since `after_id` (exclusive).
    ///
    /// `after_id`: when `Some`, passed as `page_token` so Alpaca returns only
    /// activities whose `id` is strictly after the given value (ascending order).
    /// When `None`, fetches the most recent page of FILL activities.
    ///
    /// Paginates through all available results using the same
    /// [`FILL_ACTIVITIES_PAGE_SIZE`] page loop and no-progress guard as
    /// `fetch_events`.  Returns raw [`AlpacaOrderActivity`] records — no order
    /// lookup and no [`BrokerEvent`] normalization.  Read-only.
    ///
    /// Does not check WS continuity.  The caller (repair route) is responsible
    /// for gating on gap state before invoking this method.
    pub fn fetch_fill_activities_since(
        &self,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaOrderActivity>, BrokerError> {
        let mut current_page_token: Option<String> = after_id.map(str::to_owned);
        let mut all_activities: Vec<AlpacaOrderActivity> = Vec::new();

        loop {
            let mut path = format!(
                "/v2/account/activities/FILL?direction=asc&page_size={FILL_ACTIVITIES_PAGE_SIZE}"
            );
            if let Some(token) = current_page_token.as_deref() {
                path.push_str("&page_token=");
                path.push_str(token);
            }

            let activities: Vec<AlpacaOrderActivity> = self.get(&path)?;
            let page_len = activities.len();
            let prev_page_token = current_page_token.clone();

            if let Some(last) = activities.last() {
                current_page_token = Some(last.id.clone());
            }

            if page_len == FILL_ACTIVITIES_PAGE_SIZE && current_page_token == prev_page_token {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_fill_activities_since: pagination made no progress at \
                         page_token={prev_page_token:?}; refusing to loop"
                    ),
                });
            }

            all_activities.extend(activities);

            if page_len < FILL_ACTIVITIES_PAGE_SIZE {
                break;
            }
        }

        Ok(all_activities)
    }

    // -----------------------------------------------------------------------
    // CRYPTO-FEE-ATTRIBUTION-01: day-end CFEE/FEE account-activity fetch
    // -----------------------------------------------------------------------
    /// Fetch all activities of one Alpaca fee activity type (`"CFEE"` or
    /// `"FEE"`) since `after_id` (exclusive), paginating to exhaustion.
    ///
    /// Mirrors [`Self::fetch_fill_activities_since`]'s pagination and
    /// no-progress-guard shape exactly, against the type-specific
    /// `GET /v2/account/activities/{type}` endpoint. Returns raw
    /// [`AlpacaFeeActivity`] records — no normalization, no ledger
    /// application; callers pass each record to
    /// [`fee_attribution::normalize_fee_activity`] and apply the result
    /// themselves (this crate does not own portfolio/ledger state).
    ///
    /// `activity_type` must be one of [`fee_attribution::ALPACA_FEE_ACTIVITY_TYPES`];
    /// any other value is a caller programming error, not a broker error —
    /// this function does not validate it beyond passing it into the URL.
    pub fn fetch_fee_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaFeeActivity>, BrokerError> {
        let mut current_page_token: Option<String> = after_id.map(str::to_owned);
        let mut all_activities: Vec<AlpacaFeeActivity> = Vec::new();

        loop {
            let mut path = format!(
                "/v2/account/activities/{activity_type}?direction=asc&page_size={FILL_ACTIVITIES_PAGE_SIZE}"
            );
            if let Some(token) = current_page_token.as_deref() {
                path.push_str("&page_token=");
                path.push_str(token);
            }

            let activities: Vec<AlpacaFeeActivity> = self.get(&path)?;
            let page_len = activities.len();
            let prev_page_token = current_page_token.clone();

            if let Some(last) = activities.last() {
                current_page_token = Some(last.id.clone());
            }

            if page_len == FILL_ACTIVITIES_PAGE_SIZE && current_page_token == prev_page_token {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_fee_activities_since: pagination made no progress at \
                         activity_type={activity_type:?} page_token={prev_page_token:?}; \
                         refusing to loop"
                    ),
                });
            }

            all_activities.extend(activities);

            if page_len < FILL_ACTIVITIES_PAGE_SIZE {
                break;
            }
        }

        Ok(all_activities)
    }

    // -----------------------------------------------------------------------
    // D1 correction (V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): options
    // lifecycle (OPEXC/OPASN/OPEXP/OPTRD) account-activity fetch
    // -----------------------------------------------------------------------
    /// Fetch all activities of one Alpaca options-lifecycle activity type
    /// (`"OPEXC"`, `"OPASN"`, `"OPEXP"`, or `"OPTRD"`) since `after_id`
    /// (exclusive), paginating to exhaustion.
    ///
    /// Mirrors [`Self::fetch_fee_activities_since`]'s pagination and
    /// no-progress-guard shape exactly, against the same generic
    /// `GET /v2/account/activities/{type}` endpoint. Reuses
    /// [`AlpacaFeeActivity`] as the raw wire shape -- the endpoint's JSON
    /// envelope (`id`/`activity_type`/`date`/`net_amount`/`description`/
    /// `symbol`/`qty`/`price`/`status`) is identical across activity types;
    /// only the per-type semantics of `symbol`/`qty`/`price`/`net_amount`
    /// differ, which [`option_lifecycle_normalize::normalize_option_lifecycle_activity`]
    /// interprets. Returns raw records only -- no normalization, no ledger
    /// application (this crate does not own portfolio/ledger state).
    ///
    /// `activity_type` must be one of
    /// [`option_lifecycle_normalize::ALPACA_OPTION_LIFECYCLE_ACTIVITY_TYPES`];
    /// any other value is a caller programming error, not a broker error —
    /// this function does not validate it beyond passing it into the URL.
    pub fn fetch_option_lifecycle_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaFeeActivity>, BrokerError> {
        let mut current_page_token: Option<String> = after_id.map(str::to_owned);
        let mut all_activities: Vec<AlpacaFeeActivity> = Vec::new();

        loop {
            let mut path = format!(
                "/v2/account/activities/{activity_type}?direction=asc&page_size={FILL_ACTIVITIES_PAGE_SIZE}"
            );
            if let Some(token) = current_page_token.as_deref() {
                path.push_str("&page_token=");
                path.push_str(token);
            }

            let activities: Vec<AlpacaFeeActivity> = self.get(&path)?;
            let page_len = activities.len();
            let prev_page_token = current_page_token.clone();

            if let Some(last) = activities.last() {
                current_page_token = Some(last.id.clone());
            }

            if page_len == FILL_ACTIVITIES_PAGE_SIZE && current_page_token == prev_page_token {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_option_lifecycle_activities_since: pagination made no progress at \
                         activity_type={activity_type:?} page_token={prev_page_token:?}; \
                         refusing to loop"
                    ),
                });
            }

            all_activities.extend(activities);

            if page_len < FILL_ACTIVITIES_PAGE_SIZE {
                break;
            }
        }

        Ok(all_activities)
    }

    // -----------------------------------------------------------------------
    // AP-03: Broker snapshot fetch
    // -----------------------------------------------------------------------
    /// Fetch a point-in-time broker snapshot from Alpaca.
    ///
    /// Calls three REST endpoints in sequence and normalizes the responses into
    /// the canonical [`BrokerSnapshot`] shape:
    ///
    /// | Endpoint                     | Canonical target      |
    /// |------------------------------|-----------------------|
    /// | `GET /v2/account`            | `BrokerAccount`       |
    /// | `GET /v2/positions`          | `Vec<BrokerPosition>` |
    /// | `GET /v2/orders?status=open` | `Vec<BrokerOrder>`    |
    ///
    /// `fills` is always empty.  Fill delivery is the responsibility of the
    /// `fetch_events` activity-polling path; a point-in-time snapshot cannot
    /// determine a "recent fills" window without additional context.
    ///
    /// `now_utc` is **caller-injected** so snapshot production is
    /// deterministic and testable without a live connection.
    ///
    /// # Errors
    ///
    /// Returns `Err(BrokerError)` if any endpoint call fails (transport,
    /// auth, rate-limit, transient), or if any open order carries a
    /// non-RFC-3339 `created_at` timestamp (fails closed).
    pub fn fetch_broker_snapshot(
        &self,
        now_utc: chrono::DateTime<chrono::Utc>,
    ) -> Result<BrokerSnapshot, BrokerError> {
        let account_raw: AlpacaAccountRaw = self.get("/v2/account")?;
        let positions_raw: Vec<AlpacaPositionRaw> = self.get("/v2/positions")?;
        let orders_raw: Vec<AlpacaOpenOrderRaw> = self.get("/v2/orders?status=open")?;
        let account = normalize_account(&account_raw);
        let positions = positions_raw.iter().map(normalize_position).collect();
        let orders = orders_raw
            .iter()
            .map(normalize_open_order)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(build_snapshot(now_utc, account, positions, orders))
    }

    /// Fetch read-only asset shortability metadata from Alpaca.
    ///
    /// Calls `GET /v2/assets/{symbol}`. This method does not submit, cancel,
    /// replace, or inspect orders; it is used only as a shortability preflight
    /// input for daemon policy gates.
    pub fn fetch_asset(&self, symbol: &str) -> Result<AlpacaAssetRaw, BrokerError> {
        let normalized = symbol.trim().to_ascii_uppercase();
        self.get(&format!("/v2/assets/{normalized}"))
    }

    /// D5 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
    /// submit a two-leg vertical spread as ONE `order_class=mleg` order.
    /// Refuses fail-closed before any HTTP call when
    /// `options_mleg_capability_enabled` is `false` -- the default for
    /// every checked-in constructor. Not a `BrokerAdapter` trait method:
    /// no production caller exists yet (Alpaca options capability does
    /// not exist in this codebase), mirroring D1/D2/D3's own scope.
    ///
    /// Never submits the two legs as independent orders -- `build_body`
    /// (see `mleg::build_mleg_submit_body`) always carries both legs in
    /// the single request this method sends.
    /// D5 correction: accepts only a [`VerifiedVerticalSpreadSubmission`] --
    /// constructible solely via [`mleg::build_verified_vertical_spread_request`],
    /// which re-derives and proves the frozen call/put vertical structure
    /// before any request exists at all. The compiler, not caller
    /// discipline, rules out ever reaching this HTTP call with an unproven
    /// two-symbol pair.
    pub fn submit_vertical_spread(
        &self,
        verified: &VerifiedVerticalSpreadSubmission,
    ) -> Result<SubmittedVerticalSpread, BrokerError> {
        if !self.cfg.options_mleg_capability_enabled {
            return Err(BrokerError::Reject {
                code: "options_mleg_capability_disabled".to_string(),
                detail: "submit_vertical_spread: options_mleg_capability_enabled is false -- \
                         this capability defaults off and must be explicitly opted into"
                    .to_string(),
            });
        }
        let req = verified.request();
        let body = build_mleg_submit_body(req);
        let url = format!("{}/v2/orders", self.cfg.base_url);
        let client = self.client.clone();
        let key_id = self.cfg.api_key_id.clone();
        let secret_key = self.cfg.api_secret_key.clone();
        let body_bytes = serde_json::to_vec(&body).map_err(|e| BrokerError::AmbiguousSubmit {
            detail: format!("submit_vertical_spread: body serialize error: {e}"),
        })?;
        let alpaca: AlpacaMlegSubmitResponse = Self::run_async(async move {
            let http_resp = client
                .post(&url)
                .header("APCA-API-KEY-ID", &key_id)
                .header("APCA-API-SECRET-KEY", &secret_key)
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .map_err(classify_transport_err_for_submit)?;
            let status = http_resp.status();
            if !status.is_success() {
                let retry_after_ms = parse_retry_after_ms(
                    http_resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()),
                );
                let resp_body = http_resp.text().await.unwrap_or_default();
                return Err(classify_http_status(status, &resp_body, retry_after_ms));
            }
            http_resp
                .json()
                .await
                .map_err(|e| BrokerError::AmbiguousSubmit {
                    detail: format!("submit_vertical_spread: response parse error: {e}"),
                })
        })?;
        normalize_mleg_submit_response(alpaca, req).map_err(|refusal| match refusal {
            MlegRefusal::LegCountMismatch { expected, actual } => BrokerError::AmbiguousSubmit {
                detail: format!(
                    "submit_vertical_spread: broker returned {actual} legs, expected \
                     {expected} -- the order may be live at the broker in an unproven shape"
                ),
            },
            MlegRefusal::LegIdentityMismatch { detail } => BrokerError::AmbiguousSubmit {
                detail: format!("submit_vertical_spread: {detail}"),
            },
        })
    }

    /// D5 correction: cancel of an `order_class=mleg` parent order.
    /// Re-verified live against Alpaca's current official docs (Context7,
    /// this correction): Alpaca's REST reference documents generic
    /// `DELETE /v2/orders/{id}` cancel and, at the FIX protocol level, a
    /// dedicated Multileg Order Cancel/Replace Request (`AC`) message --
    /// so mleg cancel/replace is a real, Alpaca-supported operation, not
    /// undocumented. What is NOT sufficiently documented at the REST level
    /// this crate actually speaks is a complete success-path contract for
    /// `DELETE` against an mleg PARENT id (e.g. whether both legs cancel
    /// atomically, or any per-leg partial-cancel shape) -- this method
    /// therefore still always refuses. This is a conservative MQD policy
    /// pending that proof, never a claim that Alpaca leaves mleg cancel
    /// undocumented.
    pub fn cancel_vertical_spread(&self, broker_parent_order_id: &str) -> Result<(), BrokerError> {
        Err(BrokerError::Reject {
            code: "options_mleg_cancel_unproven".to_string(),
            detail: format!(
                "cancel_vertical_spread: REST DELETE /v2/orders/{{id}} success-path semantics \
                 for order_class=mleg parent {broker_parent_order_id:?} (atomic vs. per-leg \
                 cancel) are not sufficiently proven here -- conservative MQD policy refusal, \
                 not a claim that Alpaca leaves mleg cancel undocumented"
            ),
        })
    }

    /// D5 correction: replace of an `order_class=mleg` parent order.
    /// Re-verified live against Alpaca's current official docs (Context7,
    /// this correction): `PATCH /v2/orders/{order_id}` documents
    /// mleg-specific error responses (`403 "replace mleg order is
    /// disabled"`, `422 "order chain not fully replaced"`, among others),
    /// confirming mleg replace is a real, gated Alpaca operation -- and the
    /// FIX-level Multileg Order Cancel/Replace Request (`AC`) documents a
    /// complete re-supply-all-legs contract. The REST success-path request/
    /// response shape (does REST PATCH require resupplying every leg the
    /// same way FIX `AC` does?) is not sufficiently proven here to
    /// implement narrowly and safely, so this method still always refuses
    /// -- a conservative MQD policy, never a claim that Alpaca leaves mleg
    /// replace undocumented.
    pub fn replace_vertical_spread(&self, broker_parent_order_id: &str) -> Result<(), BrokerError> {
        Err(BrokerError::Reject {
            code: "options_mleg_replace_unproven".to_string(),
            detail: format!(
                "replace_vertical_spread: REST PATCH /v2/orders/{{id}} success-path leg-resupply \
                 semantics for order_class=mleg parent {broker_parent_order_id:?} are not \
                 sufficiently proven here -- conservative MQD policy refusal, not a claim that \
                 Alpaca leaves mleg replace undocumented"
            ),
        })
    }
}
// ---------------------------------------------------------------------------
// BrokerAdapter implementation
// ---------------------------------------------------------------------------
impl BrokerAdapter for AlpacaBrokerAdapter {
    /// M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01 / M6: Alpaca is a real,
    /// wired execution venue for US equities.
    ///
    /// IR-B1-03: Crypto capability is deliberately NOT advertised yet.
    /// `BrokerAdapter::supports_asset_class` is the sole gating authority
    /// for order submission (see `mqk_execution::gateway` /
    /// `OrderRouter::broker_supports_asset_class`) — advertising Crypto
    /// support here before this adapter has canonical BTC/USD symbol
    /// mapping, min-quantity/increment validation, and fill/reconciliation
    /// identity wired would let a real crypto order reach `submit_order`
    /// through a broker-symbol/status/fill path never built or tested for
    /// it. Every asset class other than `Equity` remains refused fail-closed
    /// via the trait default. Flip Crypto to `true` here only in the
    /// coherent Alpaca-crypto completion commit, once those production
    /// seams exist and are proven by focused tests.
    fn supports_asset_class(&self, asset_class: mqk_execution::AssetClass) -> bool {
        match asset_class {
            mqk_execution::AssetClass::Equity => true,
            // D2/B4: Crypto requires BOTH the explicit capability flag AND a
            // Paper-targeting base_url -- a Live adapter can never satisfy
            // the second condition, so it can never advertise Crypto
            // regardless of the flag's value.
            mqk_execution::AssetClass::Crypto => {
                self.cfg.crypto_capability_enabled && self.cfg.targets_paper_api()
            }
            _ => false,
        }
    }

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
        if req.asset_class == AssetClass::Crypto {
            validate_alpaca_crypto_order_qty(&req.symbol, req.quantity)?;
            validate_alpaca_crypto_order_tif(&req.time_in_force)?;
        }
        let body = build_submit_body(&req);
        let url = format!("{}/v2/orders", self.cfg.base_url);
        let client = self.client.clone();
        let key_id = self.cfg.api_key_id.clone();
        let secret_key = self.cfg.api_secret_key.clone();
        // Serialize body before entering the async block so the Future is 'static.
        let body_bytes = serde_json::to_vec(&body).map_err(|e| BrokerError::AmbiguousSubmit {
            detail: format!("submit: body serialize error: {e}"),
        })?;
        Self::run_async(async move {
            let http_resp = client
                .post(&url)
                .header("APCA-API-KEY-ID", &key_id)
                .header("APCA-API-SECRET-KEY", &secret_key)
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .map_err(classify_transport_err_for_submit)?;
            let status = http_resp.status();
            if !status.is_success() {
                let retry_after_ms = parse_retry_after_ms(
                    http_resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()),
                );
                let resp_body = http_resp.text().await.unwrap_or_default();
                return Err(classify_http_status(status, &resp_body, retry_after_ms));
            }
            let alpaca: AlpacaSubmitResponse = http_resp.json().await.map_err(|e| {
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
            // Alpaca DELETE /v2/orders returns 204 No Content - no timestamp in body.
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
        // Step 1: fetch current order state to obtain filled_qty and symbol.
        let order: AlpacaOrderFull = self.fetch_order(&req.broker_order_id)?;
        // Parse filled_qty - fail closed if it is malformed.
        //
        // CUTOVER-1C Phase 2: `BrokerReplaceRequest` carries no asset-class
        // field (it is the broker-agnostic choke-point type shared by every
        // adapter), so crypto-ness is determined here from the broker's own
        // authoritative `order.symbol` — the same field already fetched for
        // this call, no additional plumbing required. A Crypto-formatted
        // symbol (contains '/', e.g. "BTC/USD") parses filled_qty as
        // fractional-capable QtyMicros; every other symbol keeps the
        // pre-existing whole-share-only fail-closed parse unchanged, so
        // equity's guard against an anomalous fractional filled_qty is not
        // weakened by this change.
        let filled_qty = if is_alpaca_crypto_symbol(&order.symbol) {
            normalize::parse_alpaca_qty_micros(&order.filled_qty).map_err(|raw| {
                BrokerError::Transient {
                    detail: format!("replace: non-parseable filled_qty from broker: {raw:?}"),
                }
            })?
        } else {
            let whole =
                parse_broker_qty(&order.filled_qty).map_err(|raw| BrokerError::Transient {
                    detail: format!("replace: non-parseable filled_qty from broker: {raw:?}"),
                })?;
            QtyMicros::from_whole_units(whole).ok_or_else(|| BrokerError::Reject {
                code: "replace_filled_qty_overflow".to_string(),
                detail: format!("replace: filled_qty={whole} overflows QtyMicros range"),
            })?
        };
        if is_alpaca_crypto_symbol(&order.symbol) {
            validate_alpaca_crypto_order_tif(&req.time_in_force)?;
        }
        // Step 2: build replace body with Alpaca total-qty semantics.
        let body = build_replace_body_for_symbol(
            req.quantity,
            filled_qty,
            req.limit_price,
            &req.time_in_force,
            &order.symbol,
        )?;
        // Step 3: send PATCH.
        let resp: AlpacaReplaceResponse =
            self.patch(&format!("/v2/orders/{}", req.broker_order_id), &body)?;
        Ok(replace_response_to_broker_response(resp))
    }
    /// Poll Alpaca for recent broker events using adapter-owned opaque resume state.
    ///
    /// BRK-00R changes the cursor contract from a raw REST activity id to a
    /// serialized `AlpacaFetchCursor`. The runtime still treats the cursor as
    /// opaque adapter-owned state.
    ///
    /// BRK-REST-02: fetch_events now paginates through all available FILL
    /// activities in one call.  The loop continues until the response contains
    /// fewer than [`FILL_ACTIVITIES_PAGE_SIZE`] activities (Alpaca's signal that
    /// the last page has been reached).  The cursor advances to the last
    /// activity's `id` field — not `transaction_time` — which Alpaca treats as
    /// an exact exclusive pagination marker on subsequent calls.
    ///
    /// Current honest coverage boundary:
    /// - REST account activities continue to provide fill / partial-fill polling.
    /// - websocket lifecycle continuity is represented explicitly in the cursor.
    /// - if websocket continuity is cold-start unproven or gap-detected,
    ///   `fetch_events` fails closed and returns `BrokerError::InboundContinuityUnproven`
    ///   with a cursor the runtime must persist.
    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        let state = decode_fetch_cursor(cursor)?;
        match &state.trade_updates {
            AlpacaTradeUpdatesResume::ColdStartUnproven => {
                return Err(continuity_fail_closed(
                    &state,
                    "alpaca trade-update continuity is cold-start unproven; REST activity polling alone cannot prove websocket lifecycle coverage",
                ));
            }
            AlpacaTradeUpdatesResume::GapDetected { detail, .. } => {
                return Err(continuity_fail_closed(
                    &state,
                    format!(
                        "alpaca trade-update continuity gap detected; lifecycle coverage remains fail-closed: {detail}"
                    ),
                ));
            }
            AlpacaTradeUpdatesResume::Live { .. } => {}
        }

        // BRK-REST-02: paginate through all available FILL activities.
        //
        // Alpaca activity pagination uses `page_token=<last_activity_id>` —
        // NOT `after=<activity_id>`.  The `after` parameter is a date-time
        // filter and must not be used as an activity-id pagination cursor.
        // `page_token` takes the last activity `id` from the previous page
        // and returns the strictly subsequent page of results.
        //
        // The durable cursor field `rest_activity_after` retains its name for
        // backward compatibility; its stored value is the last activity `id`
        // to pass as `page_token` on the next call.
        let mut current_page_token = state.rest_activity_after.clone();
        let mut all_events: Vec<BrokerEvent> = Vec::new();

        loop {
            let mut path = format!(
                "/v2/account/activities/FILL?direction=asc&page_size={FILL_ACTIVITIES_PAGE_SIZE}"
            );
            if let Some(token) = current_page_token.as_deref() {
                path.push_str("&page_token=");
                path.push_str(token);
            }

            let activities: Vec<AlpacaOrderActivity> = self.get(&path)?;
            let page_len = activities.len();

            // Record the token used for this fetch for the no-progress guard.
            let prev_page_token = current_page_token.clone();

            // Advance cursor to the last activity ID on this page.
            if let Some(last) = activities.last() {
                current_page_token = Some(last.id.clone());
            }

            // No-progress guard: if the page is full but the cursor did not
            // advance, the broker returned the same page again.  Fail closed
            // rather than spin indefinitely.
            if page_len == FILL_ACTIVITIES_PAGE_SIZE && current_page_token == prev_page_token {
                return Err(BrokerError::Transient {
                    detail: format!(
                        "fetch_events: pagination made no progress at page_token={prev_page_token:?}; \
                         refusing to loop"
                    ),
                });
            }

            // PAPER-SOAK-PARTIAL-FILL-DEDUP-04: `BrokerEvent::PartialFill::
            // cum_qty_after` now comes directly from this activity's own
            // broker-native `cum_qty` field — Alpaca's account-activities
            // endpoint reports it atomically as part of the SAME historical
            // record as this activity's `qty`/`price`/`transaction_time`.
            //
            // -02 and -03 both instead tried to RECONSTRUCT a point-in-time
            // cumulative by subtracting page quantities from a separately
            // fetched `GET /v2/orders` CURRENT total. Independent review of
            // -03 proved that approach structurally unsound in two ways no
            // amount of extra `fetch_order` reads could close:
            //
            // 1. Pre-bracket contamination: a NEW execution landing on the
            //    broker between the activities-page fetch and the
            //    reconstruction's first `fetch_order` call inflates the
            //    "current total" out from under the page's activities. A
            //    same-valued two-read bracket cannot detect this — both
            //    reads land after the new execution and agree with each
            //    other while still being wrong for this page.
            // 2. Cross-page contamination: `GET /v2/orders` returns the
            //    order's CURRENT total across ALL pages, not "as of the end
            //    of this page." Subtracting only this page's own quantity
            //    from that total silently folds in quantity from OTHER,
            //    already-executed pages, shifting every reconstructed
            //    cumulative in the page by however much later-page quantity
            //    the current total already includes — deterministically,
            //    with no race required.
            //
            // Using the activity's own `cum_qty` eliminates both failure
            // modes by construction: it is fixed at the moment the
            // execution happened, is intrinsic to that one historical
            // record, and cannot be affected by any other execution,
            // any other page, or when this record is subsequently polled.
            // No `fetch_order` call, no bracket, no page-relative
            // arithmetic is needed or used for this value. It is the
            // REST-lane structural counterpart to the WS lane's
            // `order.filled_qty` (also broker-reported atomically, in the
            // same trade-update message as the fill it describes) — both
            // lanes now source `cum_qty_after` the same way: directly from
            // the broker, in the same message as the fill itself.
            //
            // Only `PartialFill` carries a `cum_qty_after` field on
            // `BrokerEvent` — `Fill` has none. A genuine terminal fill only
            // ever occurs once per order, so the OMS's pre-existing
            // already-`Filled` idempotency (`do_transition`) already handles
            // terminal-fill redelivery correctly with no watermark needed.
            //
            // No silent ambiguity: PAPER-SOAK-PARTIAL-FILL-DEDUP-03's
            // "leave it None, fall back to event-id-only dedup" behavior for
            // an unreconstructable partial fill is not repeated here. A REST
            // FILL/partial_fill activity whose `cum_qty` cannot be trusted
            // might be an already-WS-applied duplicate under a different
            // transport id —
            // event-id-only dedup cannot tell, and applying it anyway risks
            // double-counting real capital. A missing/unparseable `cum_qty`
            // on a FILL-class activity therefore fails the WHOLE page closed
            // (`BrokerError::Transient`, consistent with this function's
            // existing behavior for any other unmappable/unparseable
            // activity in the page — see the `?` on `activity_to_trade_update`
            // and `normalize_trade_update` below) rather than emitting an
            // event with an unprovable economic identity. The cursor does
            // not advance past this page, so the broker-side activity record
            // — always durably available — is retried on the next poll; no
            // observation is lost, and no capital state is mutated on an
            // unproven basis.
            for activity in &activities {
                // Checked before any network call: cheap and avoids an
                // unnecessary `fetch_order` for an activity we are about to
                // reject anyway.
                //
                // PAPER-SOAK-ALPACA-TRADE-ACTIVITY-SCHEMA-01: the partial-fill
                // signal is `activity_type == "FILL"` combined with
                // `trade_type == "partial_fill"` (Alpaca's documented `type`
                // field) -- NOT `activity_type == "PARTIAL_FILL"`, a value
                // that never appears on the real wire. See
                // `classify_fill_subtype` for the full classification and its
                // fail-closed handling of missing/unknown subtypes.
                let unparseable_cum_qty = || BrokerError::Transient {
                    detail: format!(
                        "fetch_events: FILL/partial_fill activity id={:?} for \
                         order_id={:?} is missing a parseable broker-native \
                         cum_qty; refusing to guess cross-lane economic \
                         identity for an ambiguous fill",
                        activity.id, activity.order_id
                    ),
                };
                // Exact and syntactically valid before any network call. A
                // fractional value is only legitimate for a crypto-pair order;
                // that is decided below from the order's authoritative symbol,
                // so Equity keeps its whole-share guard.
                let partial_fill_cum_qty =
                    if classify_fill_subtype(activity).map_err(|e| BrokerError::Transient {
                        detail: format!("fetch_events: {e}"),
                    })? == Some("partial_fill")
                    {
                        let parsed = activity
                            .cum_qty
                            .as_deref()
                            .and_then(|s| normalize::parse_alpaca_qty_micros(s).ok());
                        Some(parsed.ok_or_else(unparseable_cum_qty)?)
                    } else {
                        None
                    };
                let mut order = self.fetch_order(&activity.order_id)?;
                if let Some(v) = partial_fill_cum_qty {
                    if !v.is_whole() && !is_alpaca_crypto_symbol(&order.symbol) {
                        return Err(unparseable_cum_qty());
                    }
                    order.filled_qty = v.to_string();
                }
                let trade_update = activity_to_trade_update(activity, &order).map_err(|e| {
                    BrokerError::Transient {
                        detail: format!("fetch_events: activity mapping error: {e}"),
                    }
                })?;
                let event =
                    normalize_trade_update(&trade_update).map_err(|e| BrokerError::Transient {
                        detail: format!("fetch_events: normalize error: {e}"),
                    })?;
                all_events.push(event);
            }

            // A partial (or empty) page means no more activities are available.
            if page_len < FILL_ACTIVITIES_PAGE_SIZE {
                break;
            }
        }

        let new_cursor = if current_page_token != state.rest_activity_after {
            Some(encode_fetch_cursor(&AlpacaFetchCursor {
                schema_version: state.schema_version,
                rest_activity_after: current_page_token,
                trade_updates: state.trade_updates.clone(),
            })?)
        } else {
            None
        };

        Ok((all_events, new_cursor))
    }
}

#[cfg(test)]
mod supports_asset_class_tests {
    use super::*;
    use mqk_execution::AssetClass;

    fn adapter() -> AlpacaBrokerAdapter {
        AlpacaBrokerAdapter::new_for_test("http://127.0.0.1:0".to_string())
    }

    #[test]
    fn m5_equity_is_supported() {
        let a = adapter();
        assert!(a.supports_asset_class(AssetClass::Equity));
    }

    /// IR-B1-03: Crypto must remain fail-closed/unsupported until the
    /// coherent Alpaca-crypto completion commit wires canonical BTC/USD
    /// symbol mapping, min-qty/increment validation, and fill/reconciliation
    /// identity — advertising capability ahead of that implementation would
    /// let a real order reach `submit_order` through an untested path.
    #[test]
    fn ir_b1_03_crypto_remains_unsupported_pending_alpaca_crypto_completion() {
        let a = adapter();
        assert!(!a.supports_asset_class(AssetClass::Crypto));
    }

    // --- D2/B4: explicit, default-off, Paper-only Crypto capability ---

    #[test]
    fn d2b4_default_off_for_paper() {
        let a = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "https://paper-api.alpaca.markets".to_string(),
            api_key_id: "k".to_string(),
            api_secret_key: "s".to_string(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        });
        assert!(!a.supports_asset_class(AssetClass::Crypto));
    }

    #[test]
    fn d2b4_paper_with_flag_enabled_supports_crypto() {
        let a = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "https://paper-api.alpaca.markets".to_string(),
            api_key_id: "k".to_string(),
            api_secret_key: "s".to_string(),
            crypto_capability_enabled: true,
            options_mleg_capability_enabled: false,
        });
        assert!(a.supports_asset_class(AssetClass::Crypto));
        assert!(a.crypto_capability_enabled());
    }

    #[test]
    fn d2b4_live_with_flag_enabled_still_refuses_crypto() {
        // MUTATION-RESISTANT: the flag alone must never be sufficient --
        // Live's base_url structurally blocks Crypto regardless.
        let a = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "https://api.alpaca.markets".to_string(),
            api_key_id: "k".to_string(),
            api_secret_key: "s".to_string(),
            crypto_capability_enabled: true,
            options_mleg_capability_enabled: false,
        });
        assert!(!a.supports_asset_class(AssetClass::Crypto));
    }

    #[test]
    fn d2b4_live_constructor_never_enables_crypto_capability() {
        let a = AlpacaBrokerAdapter::live("k".to_string(), "s".to_string());
        assert!(!a.crypto_capability_enabled());
        assert!(!a.supports_asset_class(AssetClass::Crypto));
    }

    #[test]
    fn d2b4_paper_constructor_defaults_off_but_can_opt_in() {
        let default_off = AlpacaBrokerAdapter::paper("k".to_string(), "s".to_string());
        assert!(!default_off.crypto_capability_enabled());
        assert!(!default_off.supports_asset_class(AssetClass::Crypto));

        let opted_in = AlpacaBrokerAdapter::paper("k".to_string(), "s".to_string())
            .with_crypto_capability_enabled(true);
        assert!(opted_in.crypto_capability_enabled());
        assert!(opted_in.supports_asset_class(AssetClass::Crypto));
    }

    #[test]
    fn m5_option_future_forex_remain_refused() {
        let a = adapter();
        assert!(!a.supports_asset_class(AssetClass::Option));
        assert!(!a.supports_asset_class(AssetClass::Future));
        assert!(!a.supports_asset_class(AssetClass::Forex));
    }
}

// ---------------------------------------------------------------------------
// CUTOVER-1C Phase 2: validate_alpaca_crypto_order_qty proof tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod crypto_qty_validation_tests {
    use super::*;

    fn qty(s: &str) -> QtyMicros {
        s.parse().unwrap()
    }

    #[test]
    fn exact_minimum_order_size_is_accepted() {
        assert!(validate_alpaca_crypto_order_qty("BTC/USD", qty("0.0001")).is_ok());
    }

    #[test]
    fn an_exact_larger_multiple_of_the_increment_is_accepted() {
        assert!(validate_alpaca_crypto_order_qty("BTC/USD", qty("0.5")).is_ok());
        assert!(validate_alpaca_crypto_order_qty("BTC/USD", qty("1.2345")).is_ok());
    }

    #[test]
    fn below_minimum_order_size_is_rejected() {
        let err = validate_alpaca_crypto_order_qty("BTC/USD", qty("0.00005")).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_qty_below_minimum"
        ));
    }

    #[test]
    fn zero_qty_is_rejected_as_below_minimum() {
        let err = validate_alpaca_crypto_order_qty("BTC/USD", QtyMicros::ZERO).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_qty_below_minimum"
        ));
    }

    #[test]
    fn a_qty_not_aligned_to_the_increment_is_rejected() {
        // 0.00015 BTC = 150 raw micros; not a multiple of the 100-raw-micro increment.
        let err = validate_alpaca_crypto_order_qty("BTC/USD", qty("0.00015")).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_qty_not_multiple_of_increment"
        ));
    }

    #[test]
    fn an_unsupported_crypto_pair_is_rejected() {
        let err = validate_alpaca_crypto_order_qty("ETH/USD", qty("1.0")).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_pair_unsupported"
        ));
    }

    #[test]
    fn submit_order_rejects_a_crypto_order_below_minimum_before_any_http_call() {
        // MockServer is intentionally not started: a network call here would
        // panic/hang, proving the reject happens before any HTTP attempt.
        let adapter = AlpacaBrokerAdapter::new_for_test("http://127.0.0.1:0".to_string());
        let token = BrokerInvokeToken::for_test();
        let req = BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "BTC/USD".to_string(),
            side: Side::Buy,
            quantity: qty("0.00001"),
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "gtc".to_string(),
            asset_class: AssetClass::Crypto,
        };
        let err = adapter.submit_order(req, &token).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_qty_below_minimum"
        ));
    }

    #[test]
    fn submit_order_does_not_apply_crypto_qty_validation_to_equity_orders() {
        // An equity order with a "sub-minimum-if-it-were-crypto" quantity must
        // not be rejected by this gate -- it is asset-class gated, not blanket.
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(200).json_body(serde_json::json!({
                "id": "broker-order-1",
                "client_order_id": "ord-1",
                "created_at": "2024-01-01T00:00:00Z"
            }));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let token = BrokerInvokeToken::for_test();
        let req = BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            quantity: QtyMicros::from_whole_units(10).unwrap(),
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
            asset_class: AssetClass::Equity,
        };
        assert!(adapter.submit_order(req, &token).is_ok());
        mock.assert_hits(1);
    }
}

// ---------------------------------------------------------------------------
// CUTOVER-1C Phase 2: replace_order fractional filled_qty proof tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod crypto_replace_fractional_filled_qty_tests {
    use super::*;
    use httpmock::prelude::*;
    use httpmock::Method::PATCH;

    #[test]
    fn is_alpaca_crypto_symbol_distinguishes_pairs_from_tickers() {
        assert!(is_alpaca_crypto_symbol("BTC/USD"));
        assert!(!is_alpaca_crypto_symbol("AAPL"));
    }

    /// A crypto order that already has a fractional broker-reported
    /// filled_qty must no longer fail closed on replace -- CUTOVER-1C Phase 2
    /// closes the gap `build_replace_body`'s prior doc comment documented
    /// ("a fractional Crypto order that already has a fractional fill cannot
    /// reach this function at all yet").
    #[test]
    fn replace_order_accepts_a_crypto_order_with_fractional_filled_qty() {
        let server = MockServer::start();
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/v2/orders/btc-order-1");
            then.status(200).json_body(serde_json::json!({
                "id": "btc-order-1",
                "client_order_id": "internal-1",
                "symbol": "BTC/USD",
                "side": "buy",
                "qty": "0.0001",
                "filled_qty": "0.0001"
            }));
        });
        // Total sent to Alpaca must be filled (0.0001) + new leaves (0.0002) = 0.0003.
        let patch_mock = server.mock(|when, then| {
            when.method(PATCH)
                .path("/v2/orders/btc-order-1")
                .json_body_partial(r#"{"qty": "0.0003"}"#);
            then.status(200).json_body(serde_json::json!({
                "id": "btc-order-1-replaced",
                "qty": "0.0003"
            }));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let token = BrokerInvokeToken::for_test();
        let req = BrokerReplaceRequest {
            broker_order_id: "btc-order-1".to_string(),
            quantity: "0.0002".parse().unwrap(),
            limit_price: None,
            time_in_force: "gtc".to_string(),
        };
        let resp = adapter
            .replace_order(req, &token)
            .expect("replace of a fractionally-filled crypto order must succeed, not fail closed");
        assert_eq!(resp.broker_order_id, "btc-order-1-replaced");
        get_mock.assert_hits(1);
        patch_mock.assert_hits(1);
    }

    /// Negative control: an Equity order with a malformed/fractional
    /// filled_qty must still fail closed exactly as before this patch --
    /// widening the crypto path must not weaken the equity guard.
    #[test]
    fn replace_order_still_fails_closed_on_fractional_filled_qty_for_equity() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/orders/eq-order-1");
            then.status(200).json_body(serde_json::json!({
                "id": "eq-order-1",
                "client_order_id": "internal-2",
                "symbol": "AAPL",
                "side": "buy",
                "qty": "100",
                "filled_qty": "30.5"
            }));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let token = BrokerInvokeToken::for_test();
        let req = BrokerReplaceRequest {
            broker_order_id: "eq-order-1".to_string(),
            quantity: QtyMicros::from_whole_units(20).unwrap(),
            limit_price: None,
            time_in_force: "day".to_string(),
        };
        let err = adapter.replace_order(req, &token).unwrap_err();
        assert!(
            matches!(err, BrokerError::Transient { .. }),
            "an anomalous fractional filled_qty for an Equity symbol must still fail closed: {err:?}"
        );
    }

    /// RC-M6-A: a crypto replace with a time-in-force Alpaca does not support
    /// is refused before the PATCH; the equity replace above keeps `day`.
    #[test]
    fn replace_order_refuses_unsupported_crypto_time_in_force_before_patch() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/orders/btc-order-2");
            then.status(200).json_body(serde_json::json!({
                "id": "btc-order-2",
                "client_order_id": "internal-3",
                "symbol": "BTC/USD",
                "side": "buy",
                "qty": "0.0001",
                "filled_qty": "0"
            }));
        });
        let patch_mock = server.mock(|when, then| {
            when.method(PATCH).path("/v2/orders/btc-order-2");
            then.status(200).json_body(serde_json::json!({"id": "x"}));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let token = BrokerInvokeToken::for_test();
        let req = BrokerReplaceRequest {
            broker_order_id: "btc-order-2".to_string(),
            quantity: "0.0002".parse().unwrap(),
            limit_price: None,
            time_in_force: "day".to_string(),
        };
        let err = adapter.replace_order(req, &token).unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "crypto_time_in_force_unsupported"
        ));
        patch_mock.assert_hits(0);
    }
}

#[cfg(test)]
mod crypto_time_in_force_validation_tests {
    use super::*;

    #[test]
    fn gtc_and_ioc_are_accepted_case_insensitively() {
        for tif in ["gtc", "ioc", "GTC", " Ioc "] {
            assert!(validate_alpaca_crypto_order_tif(tif).is_ok(), "{tif:?}");
        }
    }

    #[test]
    fn every_other_time_in_force_is_refused() {
        for tif in ["day", "fok", "opg", "cls", "gtd", ""] {
            let err = validate_alpaca_crypto_order_tif(tif).unwrap_err();
            assert!(
                matches!(
                    err,
                    BrokerError::Reject { ref code, .. } if code == "crypto_time_in_force_unsupported"
                ),
                "{tif:?}: {err:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CRYPTO-FEE-ATTRIBUTION-01: fetch_fee_activities_since proof tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod fetch_fee_activities_since_tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn single_page_of_cfee_activities_is_returned_in_full() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v2/account/activities/CFEE")
                .query_param("direction", "asc");
            then.status(200).json_body(serde_json::json!([
                {
                    "id": "20220812000000000::53be51ba-46f9-43de-b81f-576f241dc680",
                    "activity_type": "CFEE",
                    "date": "2022-08-12",
                    "net_amount": "-0.01",
                    "description": "Coin Pair Transaction Fee (Non USD)",
                    "symbol": "BTCUSD",
                    "qty": "0",
                    "price": "50000",
                    "status": "executed"
                }
            ]));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let activities = adapter
            .fetch_fee_activities_since("CFEE", None)
            .expect("fee activity fetch must succeed");
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].net_amount, "-0.01");
        mock.assert_hits(1);
    }

    #[test]
    fn empty_page_returns_no_activities() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/account/activities/FEE");
            then.status(200).json_body(serde_json::json!([]));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let activities = adapter
            .fetch_fee_activities_since("FEE", None)
            .expect("fee activity fetch must succeed");
        assert!(activities.is_empty());
    }

    #[test]
    fn after_id_is_threaded_through_as_page_token() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v2/account/activities/CFEE")
                .query_param("page_token", "prior-activity-id");
            then.status(200).json_body(serde_json::json!([]));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        adapter
            .fetch_fee_activities_since("CFEE", Some("prior-activity-id"))
            .expect("fee activity fetch must succeed");
        mock.assert_hits(1);
    }
}

// ---------------------------------------------------------------------------
// B6: provider account identity is GET /v2/account `id`, never the API key id
// ---------------------------------------------------------------------------
#[cfg(test)]
mod provider_account_identity_tests {
    use super::*;
    use httpmock::prelude::*;

    const ACCOUNT_UUID: &str = "e6fe16f3-64a4-4921-8928-cadf02f92f98";

    fn adapter_with_key(base_url: String, key_id: &str) -> AlpacaBrokerAdapter {
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id.to_string(),
            api_secret_key: "test-secret".to_string(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
    }

    fn mock_account(server: &MockServer, body: serde_json::Value) {
        server.mock(|when, then| {
            when.method(GET).path("/v2/account");
            then.status(200).json_body(body);
        });
    }

    #[test]
    fn rotated_credential_on_the_same_account_yields_the_same_identity() {
        let server = MockServer::start();
        mock_account(
            &server,
            serde_json::json!({"id": ACCOUNT_UUID, "account_number": "010203ABCD", "status": "ACTIVE"}),
        );
        let before = adapter_with_key(server.base_url(), "PK-KEY-BEFORE-ROTATION")
            .fetch_provider_account_id()
            .unwrap();
        let after = adapter_with_key(server.base_url(), "PK-KEY-AFTER-ROTATION")
            .fetch_provider_account_id()
            .unwrap();
        assert_eq!(before, ACCOUNT_UUID);
        assert_eq!(before, after);
        assert_ne!(
            before, "PK-KEY-BEFORE-ROTATION",
            "identity is never the key id"
        );
    }

    #[test]
    fn two_provider_accounts_yield_two_identities() {
        let a = MockServer::start();
        let b = MockServer::start();
        mock_account(&a, serde_json::json!({"id": ACCOUNT_UUID}));
        mock_account(
            &b,
            serde_json::json!({"id": "9feee08f-22d2-4804-89c1-bf01166aad52"}),
        );
        let same_key = "PK-SAME-KEY-TEXT";
        assert_ne!(
            adapter_with_key(a.base_url(), same_key)
                .fetch_provider_account_id()
                .unwrap(),
            adapter_with_key(b.base_url(), same_key)
                .fetch_provider_account_id()
                .unwrap(),
        );
    }

    #[test]
    fn missing_blank_or_malformed_account_id_fails_closed_never_falls_back_to_the_key() {
        for body in [
            serde_json::json!({"account_number": "010203ABCD"}),
            serde_json::json!({"id": ""}),
            serde_json::json!({"id": "   "}),
            serde_json::json!({"id": null}),
            serde_json::json!({"id": "has space"}),
            serde_json::json!({"id": "colon:injected"}),
        ] {
            let server = MockServer::start();
            mock_account(&server, body.clone());
            let err = adapter_with_key(server.base_url(), "PK-SOME-KEY")
                .fetch_provider_account_id()
                .expect_err(&format!("{body} must refuse"));
            assert!(matches!(err, BrokerError::Reject { .. }), "{err:?}");
        }
    }

    #[test]
    fn transport_failure_is_an_error_not_an_identity() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/account");
            then.status(500);
        });
        assert!(adapter_with_key(server.base_url(), "PK-SOME-KEY")
            .fetch_provider_account_id()
            .is_err());
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
    let limit_price = req.limit_price.map(|price_micros| {
        if req.asset_class == AssetClass::Crypto {
            format_alpaca_crypto_price(price_micros)
        } else {
            format_alpaca_price(price_micros)
        }
    });
    AlpacaSubmitBody {
        symbol: req.symbol.clone(),
        qty: format_alpaca_qty(req.quantity),
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
/// count, fractional-capable (QTY-MICROS-PRODUCTION-CUTOVER-01) for Crypto.
/// `filled_qty` is fractional-capable `QtyMicros` (CUTOVER-1C Phase 2): the
/// caller (`replace_order`) parses it as whole-share-only for Equity symbols
/// and as fractional for Crypto symbols, converting either representation to
/// `QtyMicros` before calling this function -- see that call site's doc
/// comment. This function computes `new_total = filled_qty + new_leaves_qty`,
/// failing closed on overflow rather than wrapping.
pub fn build_replace_body(
    new_leaves_qty: QtyMicros,
    filled_qty: QtyMicros,
    limit_price: Option<i64>,
    time_in_force: &str,
) -> Result<AlpacaReplaceBody, BrokerError> {
    let new_total_qty =
        filled_qty
            .checked_add(new_leaves_qty)
            .ok_or_else(|| BrokerError::Reject {
                code: "replace_quantity_overflow".to_string(),
                detail: format!(
                    "replace: filled_qty={filled_qty} + new_leaves_qty={new_leaves_qty} overflows"
                ),
            })?;
    Ok(AlpacaReplaceBody {
        qty: format_alpaca_qty(new_total_qty),
        limit_price: limit_price.map(format_alpaca_price),
        time_in_force: time_in_force.to_string(),
    })
}
/// Build the production Alpaca replace body with asset-aware exact
/// price serialization.
///
/// `replace_order` already fetched the broker's authoritative order symbol.
/// Crypto pair symbols therefore use [`format_alpaca_crypto_price`], while
/// bare Equity tickers retain the existing [`format_alpaca_price`] behavior.
///
/// Price-tick validation is intentionally not duplicated here. The canonical
/// order-economics gate owns that authorization decision.
fn build_replace_body_for_symbol(
    new_leaves_qty: QtyMicros,
    filled_qty: QtyMicros,
    limit_price: Option<i64>,
    time_in_force: &str,
    symbol: &str,
) -> Result<AlpacaReplaceBody, BrokerError> {
    let mut body = build_replace_body(new_leaves_qty, filled_qty, None, time_in_force)?;

    body.limit_price = limit_price.map(|price_micros| {
        if is_alpaca_crypto_symbol(symbol) {
            format_alpaca_crypto_price(price_micros)
        } else {
            format_alpaca_price(price_micros)
        }
    });

    Ok(body)
}

/// True when an Alpaca symbol string is in canonical crypto-pair wire format
/// (contains `/`, e.g. `"BTC/USD"`) rather than a bare equity ticker (e.g.
/// `"AAPL"`). Used by [`AlpacaBrokerAdapter::replace_order`] to decide
/// whether a broker-reported `filled_qty` may legitimately be fractional.
fn is_alpaca_crypto_symbol(symbol: &str) -> bool {
    symbol.contains('/')
}
/// Convert Alpaca's `PATCH /v2/orders/{id}` response into a `BrokerReplaceResponse`.
///
/// # Alpaca replace-creates-a-new-order semantics
///
/// Alpaca's replace endpoint does not amend the existing order in place — it
/// cancels the original and creates a **new** broker order with a new UUID.
/// `AlpacaReplaceResponse.id` carries that new id, distinct from the
/// `broker_order_id` used in the PATCH URL. The response's `id` — not the
/// pre-replace request id — is the current, live broker order identity going
/// forward; using the stale request id here would silently point every
/// downstream consumer (order-map updates, future cancel/replace calls,
/// reconciliation) at a broker order that no longer represents the live order.
pub fn replace_response_to_broker_response(resp: AlpacaReplaceResponse) -> BrokerReplaceResponse {
    BrokerReplaceResponse {
        broker_order_id: resp.id,
        // Alpaca PATCH does not guarantee a timestamp in the response.
        replaced_at: 0,
        status: "replace_requested".to_string(),
    }
}
/// Classify an Alpaca REST `TradeActivity`'s fill subtype
/// (PAPER-SOAK-ALPACA-TRADE-ACTIVITY-SCHEMA-01).
///
/// Per Alpaca's documented Account Activities schema, `activity_type` is
/// always `"FILL"` for a trade activity; the execution subtype -- terminal
/// vs. partial -- is carried by the separate `type` field (mapped to
/// `AlpacaOrderActivity::trade_type` because `type` is a Rust keyword):
/// `"fill"` or `"partial_fill"`.
///
/// Returns:
/// - `Ok(Some("fill"))` / `Ok(Some("partial_fill"))` for a FILL-class
///   activity with a recognized subtype.
/// - `Ok(None)` for a non-FILL-class activity (`NEW`, `CANCELED`, etc.) --
///   `trade_type` is not meaningful for these and the caller's existing
///   `activity_type`-based routing applies unchanged.
/// - `Err(String)` for a FILL-class activity whose subtype is missing or
///   unrecognized -- fail closed rather than guess. A prior implementation
///   inferred a partial fill from `activity_type == "PARTIAL_FILL"`, a value
///   that does not appear in Alpaca's documented wire shape; this made the
///   production partial-fill classification path unreachable.
pub fn classify_fill_subtype(
    activity: &AlpacaOrderActivity,
) -> Result<Option<&'static str>, String> {
    if activity.activity_type != "FILL" {
        return Ok(None);
    }
    match activity.trade_type.as_deref() {
        Some("partial_fill") => Ok(Some("partial_fill")),
        Some("fill") => Ok(Some("fill")),
        Some(other) => Err(format!(
            "classify_fill_subtype: FILL activity id={:?} order_id={:?} has unsupported type: {other:?}",
            activity.id, activity.order_id
        )),
        None => Err(format!(
            "classify_fill_subtype: FILL activity id={:?} order_id={:?} is missing the required type field",
            activity.id, activity.order_id
        )),
    }
}
/// Convert an `AlpacaOrderActivity` (REST polling) into an `AlpacaTradeUpdate`
/// (normalizer input), given the full order state from a parallel order lookup.
///
/// Only `"FILL"` (with a recognized `trade_type` subtype) and the
/// non-fill lifecycle activity types are supported.
///
/// The activity `id` is mapped to `broker_fill_id` so downstream consumers can
/// treat it as strong broker-native economic fill identity.
///
/// # Errors
///
/// Returns `Err(String)` if `activity.activity_type` is not a recognised
/// activity class, or if it is `"FILL"` with a missing/unrecognized
/// `trade_type` subtype -- see [`classify_fill_subtype`].
pub fn activity_to_trade_update(
    activity: &AlpacaOrderActivity,
    order: &AlpacaOrderFull,
) -> Result<AlpacaTradeUpdate, String> {
    // Map Alpaca uppercase activity_type to normalizer event string.
    let event_type = match activity.activity_type.as_str() {
        "NEW" | "PENDING_NEW" | "ACCEPTED" => "new",
        "FILL" => classify_fill_subtype(activity)?.expect("activity_type == FILL implies Some"),
        "CANCELED" | "EXPIRED" => "canceled",
        "CANCEL_REJECTED" => "cancel_rejected",
        "REPLACED" => "replaced",
        "REPLACE_REJECTED" => "replace_rejected",
        "REJECTED" => "rejected",
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
        broker_fill_id: Some(activity.id.clone()),
    })
}
pub fn decode_fetch_cursor(cursor: Option<&str>) -> Result<AlpacaFetchCursor, BrokerError> {
    match cursor {
        None => Ok(AlpacaFetchCursor::cold_start_unproven(None)),
        Some(raw) if raw.trim_start().starts_with('{') => {
            serde_json::from_str(raw).map_err(|e| BrokerError::Transient {
                detail: format!("fetch_events: invalid alpaca cursor state: {e}"),
            })
        }
        Some(raw) => Ok(AlpacaFetchCursor::cold_start_unproven(Some(
            raw.to_string(),
        ))),
    }
}
pub fn encode_fetch_cursor(cursor: &AlpacaFetchCursor) -> Result<String, BrokerError> {
    serde_json::to_string(cursor).map_err(|e| BrokerError::Transient {
        detail: format!("fetch_events: failed to serialize alpaca cursor state: {e}"),
    })
}
fn continuity_fail_closed(cursor: &AlpacaFetchCursor, detail: impl Into<String>) -> BrokerError {
    let persist_cursor = encode_fetch_cursor(cursor).ok();
    BrokerError::InboundContinuityUnproven {
        detail: detail.into(),
        persist_cursor,
    }
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
/// Format an order quantity for the Alpaca REST wire.
///
/// **Equity default: whole-share integer string.**  `qty` is the canonical
/// positive `i64` from `BrokerSubmitRequest.quantity` or the computed
/// total-quantity in replace semantics.
///
/// `i64::to_string()` never produces scientific notation or floating-point
/// artifacts, so this function is deterministic for all valid inputs.
///
/// # Equity invariant
/// The current execution model carries quantity as a whole-share `i64`.
/// Fractional quantities are structurally not expressible through
/// `BrokerSubmitRequest.quantity: i64` and are not claimed supported here.
///
/// # TODO BRK-PRICE-01: multi-asset extension point
/// QTY-MICROS-PRODUCTION-CUTOVER-01: renders via `QtyMicros`'s canonical
/// decimal `Display` -- an exact whole-unit value (equity shares) renders
/// as a bare integer string ("10"), unchanged from the prior behavior;
/// a fractional value (Crypto, e.g. 0.5 BTC) renders as a trimmed decimal
/// string ("0.5"), which is Alpaca's documented wire format for fractional
/// quantity. Adapter-specific increment/minimum enforcement (e.g. Alpaca's
/// BTC/USD minimum increment) happens upstream of this formatter, not here.
pub fn format_alpaca_qty(qty: QtyMicros) -> String {
    qty.to_string()
}

/// Format a price in integer micros for the Alpaca REST wire.
///
/// **Equity default: 2 decimal places (cent precision).**  This covers all
/// standard US equity and ETF limit prices.  The `{:.2}` formatter rounds
/// to the nearest cent — sub-cent precision stored in micros (e.g.
/// `150_505_000` = $150.505) rounds to `"150.51"`.
///
/// # Rounding policy
/// For US equities the minimum tick is $0.01 (100_000 micros).  Sub-cent
/// micros on the submit path indicate a data-quality error upstream.  The
/// rounding here is **explicit and deterministic**, not silent: callers can
/// verify the round-trip via `price_to_micros(format_alpaca_price(m))`.
///
/// # TODO BRK-PRICE-01: multi-asset extension point
/// When `BrokerSubmitRequest` gains `instrument: Instrument`, select
/// precision by `ContractSpec`:
/// - `Equity`: 2dp (current)
/// - `Future`: `tick_size_micros` from `ContractSpec::Future`
/// - `Crypto`: asset-specific decimal precision (often 2–8dp)
/// - `Option`: $0.01 increments (same as equity for equity options)
pub fn format_alpaca_price(micros: i64) -> String {
    // micros_to_price returns f64; format to 2 decimal places for US equity wire.
    format!("{:.2}", micros_to_price(micros))
}

/// Format a Crypto price from canonical price micros as an exact decimal.
///
/// BRK-PRICE-01A: Crypto must never pass through the Equity `f64`/2dp
/// formatter. This function serializes the canonical integer-micros value
/// exactly, trimming only insignificant trailing fractional zeroes.
///
/// It does NOT decide whether a price is on the instrument's valid price
/// tick. That authority belongs to the canonical instrument-economics gate.
///
/// Examples:
/// - 60_000_000_000 -> "60000"
/// - 60_000_500_000 -> "60000.5"
/// - 1 -> "0.000001"
pub fn format_alpaca_crypto_price(micros: i64) -> String {
    let value = micros as i128;
    let negative = value < 0;
    let absolute = value.abs();

    let whole = absolute / 1_000_000;
    let fraction = absolute % 1_000_000;

    let mut rendered = if fraction == 0 {
        whole.to_string()
    } else {
        let mut fraction_text = format!("{fraction:06}");

        while fraction_text.ends_with('0') {
            fraction_text.pop();
        }

        format!("{whole}.{fraction_text}")
    };

    if negative {
        rendered.insert(0, '-');
    }

    rendered
}

/// Convert integer micros to a decimal price string for the Alpaca broker wire.
///
/// **Backward-compatibility alias for [`format_alpaca_price`].**
/// New code should call `format_alpaca_price` directly.
pub fn micros_to_price_str(micros: i64) -> String {
    format_alpaca_price(micros)
}
/// Parse a broker decimal quantity string (e.g. `"100.000000"`) to `i64`.
///
/// Delegates to [`normalize::parse_alpaca_whole_share_qty`] — the ONE
/// canonical whole-share parser for this crate. Returns `Err(raw)` (fails
/// closed, does not round) if the string is not an integral, finite,
/// non-negative number.
fn parse_broker_qty(raw: &str) -> Result<i64, &str> {
    normalize::parse_alpaca_whole_share_qty(raw).map_err(|_| raw)
}
/// Parse an ISO 8601 timestamp to Unix epoch milliseconds.
fn parse_iso_to_epoch_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64) // allow: ops-metadata - converts broker-supplied ISO timestamp to cursor ms; not a wall-clock read
}
/// Read the async HTTP response: return parsed JSON on success, or classify
/// the error on failure.
async fn parse_success_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, BrokerError> {
    let status = resp.status();
    if status.is_success() {
        resp.json::<T>().await.map_err(|e| BrokerError::Transient {
            detail: format!("response parse error: {e}"),
        })
    } else {
        let retry_after_ms = parse_retry_after_ms(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );
        let body = resp.text().await.unwrap_or_default();
        Err(classify_http_status(status, &body, retry_after_ms))
    }
}
/// Parse a `Retry-After` response header value as milliseconds. Alpaca (like
/// standard HTTP) sends this as an integer count of seconds; missing or
/// malformed values return `None` rather than failing the request.
fn parse_retry_after_ms(value: Option<&str>) -> Option<u64> {
    let secs: u64 = value?.trim().parse().ok()?;
    secs.checked_mul(1000)
}
/// Map a `reqwest::Error` to `BrokerError` for **submit** calls.
///
/// For submit, any error that is not a clean connection refusal is treated
/// as `AmbiguousSubmit` because the order may have reached the broker.
fn classify_transport_err_for_submit(err: reqwest::Error) -> BrokerError {
    if err.is_connect() {
        // Connection refused before the request was sent - safe to retry.
        BrokerError::Transport {
            non_delivery_proven: true,
            detail: err.to_string(),
        }
    } else {
        // Timeout, builder error, or mid-flight failure - order may be live.
        BrokerError::AmbiguousSubmit {
            detail: err.to_string(),
        }
    }
}
/// Map a `reqwest::Error` to `BrokerError` for all non-submit calls.
fn classify_transport_err(err: reqwest::Error) -> BrokerError {
    if err.is_connect() {
        BrokerError::Transport {
            non_delivery_proven: true,
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
///
/// `retry_after_ms`, when the response carried a parseable `Retry-After`
/// header, is threaded onto `BrokerError::RateLimit` so the orchestrator's
/// existing outbox-driven redispatch can honor the broker's actual timing
/// guidance instead of guessing a fixed delay. This function itself never
/// retries anything -- it only classifies.
fn classify_http_status(
    status: reqwest::StatusCode,
    body: &str,
    retry_after_ms: Option<u64>,
) -> BrokerError {
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
            retry_after_ms,
            non_delivery_proven: true,
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
// ---------------------------------------------------------------------------
// Unit tests: classify_fill_subtype (PAPER-SOAK-ALPACA-TRADE-ACTIVITY-SCHEMA-01)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod classify_fill_subtype_tests {
    use super::*;

    fn activity(activity_type: &str, trade_type: Option<&str>) -> AlpacaOrderActivity {
        AlpacaOrderActivity {
            id: "activity-1".to_string(),
            activity_type: activity_type.to_string(),
            trade_type: trade_type.map(str::to_string),
            order_id: "order-1".to_string(),
            transaction_time: "2024-01-15T10:30:00Z".to_string(),
            price: Some("100.00".to_string()),
            qty: Some("10".to_string()),
            side: "buy".to_string(),
            symbol: "AAPL".to_string(),
            cum_qty: Some("10".to_string()),
        }
    }

    // S01 (deserialization contract): real-shape JSON with both fields.
    #[test]
    fn s01_real_shape_partial_fill_deserializes_both_fields() {
        let json = serde_json::json!({
            "id": "activity-1",
            "activity_type": "FILL",
            "type": "partial_fill",
            "order_id": "order-1",
            "transaction_time": "2024-01-15T10:30:00Z",
            "price": "100.00",
            "qty": "10",
            "cum_qty": "10",
            "side": "buy",
            "symbol": "AAPL"
        });
        let parsed: AlpacaOrderActivity = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.activity_type, "FILL");
        assert_eq!(parsed.trade_type.as_deref(), Some("partial_fill"));
    }

    #[test]
    fn fill_class_partial_fill_type_classifies_as_partial_fill() {
        let a = activity("FILL", Some("partial_fill"));
        assert_eq!(classify_fill_subtype(&a).unwrap(), Some("partial_fill"));
    }

    // S03: activity_type=FILL, type=fill classifies as terminal fill.
    #[test]
    fn fill_class_fill_type_classifies_as_fill() {
        let a = activity("FILL", Some("fill"));
        assert_eq!(classify_fill_subtype(&a).unwrap(), Some("fill"));
    }

    #[test]
    fn non_fill_activity_type_classifies_as_none() {
        let a = activity("NEW", None);
        assert_eq!(classify_fill_subtype(&a).unwrap(), None);
    }

    // S09: FILL activity missing the type field fails closed, not a default fill.
    #[test]
    fn s09_fill_class_missing_type_fails_closed() {
        let a = activity("FILL", None);
        let err = classify_fill_subtype(&a).unwrap_err();
        assert!(err.contains("missing"), "unexpected error: {err}");
    }

    // S10: FILL activity with an unrecognized type value fails closed.
    #[test]
    fn s10_fill_class_unknown_type_fails_closed() {
        let a = activity("FILL", Some("unexpected_value"));
        let err = classify_fill_subtype(&a).unwrap_err();
        assert!(err.contains("unexpected_value"), "unexpected error: {err}");
    }

    #[test]
    fn fill_class_empty_type_fails_closed() {
        let a = activity("FILL", Some(""));
        assert!(classify_fill_subtype(&a).is_err());
    }
}
// ---------------------------------------------------------------------------
// Unit tests: Retry-After parsing + threading (BROKER-ALPACA-RATE-LIMIT-
// RETRY-AFTER-01). Scope is strictly the parser and its threading onto
// BrokerError::RateLimit -- no retry/backoff policy change of any kind.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod broker_retry_tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Pure helpers
    // -----------------------------------------------------------------------

    #[test]
    fn br01_retry_after_parses_seconds_to_ms_and_falls_back_on_malformed() {
        assert_eq!(parse_retry_after_ms(Some("5")), Some(5_000));
        assert_eq!(parse_retry_after_ms(Some(" 5 ")), Some(5_000));
        assert_eq!(parse_retry_after_ms(Some("not-a-number")), None);
        assert_eq!(parse_retry_after_ms(Some("")), None);
        assert_eq!(parse_retry_after_ms(None), None);
    }

    // BR-02: a seconds value whose ms conversion overflows u64 must return
    // None rather than panic (checked_mul, not a bare multiply).
    #[test]
    fn br02_retry_after_overflowing_seconds_to_ms_returns_none_no_panic() {
        assert_eq!(parse_retry_after_ms(Some(&u64::MAX.to_string())), None);
    }

    // BR-03: classify_http_status threads a parsed Retry-After onto
    // RateLimit; other variants ignore it (their shape carries no such
    // field) -- non-429 classification is unchanged by this patch.
    #[test]
    fn br03_classify_http_status_threads_retry_after_onto_rate_limit_only() {
        let err = classify_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "slow down",
            Some(30_000),
        );
        assert_eq!(
            err,
            BrokerError::RateLimit {
                retry_after_ms: Some(30_000),
                non_delivery_proven: true,
                detail: "slow down".to_string(),
            }
        );

        let err_no_header =
            classify_http_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "slow down", None);
        assert_eq!(
            err_no_header,
            BrokerError::RateLimit {
                retry_after_ms: None,
                non_delivery_proven: true,
                detail: "slow down".to_string(),
            }
        );

        // A retry_after_ms value is passed through unused for a status
        // whose BrokerError variant carries no such field -- classification
        // itself is otherwise identical to before this patch.
        let err_500 = classify_http_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "oops",
            Some(5_000),
        );
        assert!(matches!(err_500, BrokerError::Transient { .. }));
    }

    // -----------------------------------------------------------------------
    // get() Retry-After threading via httpmock -- single attempt only, no
    // retry of any kind.
    // -----------------------------------------------------------------------

    // BR-04: a 429 with a valid Retry-After header surfaces the correctly
    // parsed retry_after_ms on the very first (and only) attempt.
    #[test]
    fn br04_get_429_with_valid_retry_after_surfaces_parsed_ms() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/assets/AAPL");
            then.status(429)
                .header("Retry-After", "5")
                .body("rate limited");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let err = adapter.fetch_asset("AAPL").unwrap_err();
        assert_eq!(
            err,
            BrokerError::RateLimit {
                retry_after_ms: Some(5_000),
                non_delivery_proven: true,
                detail: "rate limited".to_string(),
            }
        );
        mock.assert_hits(1);
    }

    // BR-05: a 429 with no Retry-After header surfaces retry_after_ms=None
    // -- absent guidance is reported truthfully, never guessed.
    #[test]
    fn br05_get_429_without_retry_after_header_surfaces_none() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/assets/AAPL");
            then.status(429).body("rate limited");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let err = adapter.fetch_asset("AAPL").unwrap_err();
        match err {
            BrokerError::RateLimit { retry_after_ms, .. } => assert_eq!(retry_after_ms, None),
            other => panic!("expected RateLimit, got {other:?}"),
        }
        mock.assert_hits(1);
    }

    // BR-06: a 429 with a malformed Retry-After header surfaces
    // retry_after_ms=None rather than failing the request outright.
    #[test]
    fn br06_get_429_with_malformed_retry_after_surfaces_none() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/assets/AAPL");
            then.status(429)
                .header("Retry-After", "not-a-number")
                .body("rate limited");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let err = adapter.fetch_asset("AAPL").unwrap_err();
        match err {
            BrokerError::RateLimit { retry_after_ms, .. } => assert_eq!(retry_after_ms, None),
            other => panic!("expected RateLimit, got {other:?}"),
        }
        mock.assert_hits(1);
    }

    // BR-07: a permanent 4xx read (404) is classified as Reject exactly as
    // before this patch -- non-429 status behavior is unchanged.
    #[test]
    fn br07_permanent_4xx_read_classification_unchanged() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/assets/UNKNOWN");
            then.status(404).body("not found");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let err = adapter.fetch_asset("UNKNOWN").unwrap_err();
        assert!(matches!(err, BrokerError::Reject { .. }));
        mock.assert_hits(1);
    }

    // BR-08: a retryable status on a read (GET) is never retried in-adapter
    // -- exactly one attempt. This is the negative control proving R2-P3
    // introduces no new automatic retry behavior anywhere, on the one call
    // site the rejected implementation had looped.
    #[test]
    fn br08_get_429_is_never_retried_in_adapter() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/assets/AAPL");
            then.status(429).body("rate limited");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let err = adapter.fetch_asset("AAPL").unwrap_err();
        assert!(matches!(err, BrokerError::RateLimit { .. }));
        mock.assert_hits(1);
    }

    // BR-09: mutating calls (DELETE / cancel) are never retried in-adapter,
    // even on a retryable status -- exactly one attempt, surfaced as
    // RateLimit for the orchestrator's own redispatch authority to own.
    // Request counts for submit/patch/delete are otherwise unaffected by
    // this patch, which only threads Retry-After parsing onto their
    // existing (unlooped) single-attempt call sites.
    #[test]
    fn br09_cancel_429_is_never_retried_in_adapter() {
        use httpmock::prelude::*;
        use mqk_execution::{BrokerAdapter, BrokerInvokeToken};

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE).path("/v2/orders/order-1");
            then.status(429).body("rate limited");
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url());
        let token = BrokerInvokeToken::for_test();
        let err = adapter.cancel_order("order-1", &token).unwrap_err();
        assert!(matches!(err, BrokerError::RateLimit { .. }));
        mock.assert_hits(1);
    }
}

// ---------------------------------------------------------------------------
// D5: multi-leg vertical-spread submission -- capability gate, single-POST
// shape, and cancel/replace fail-closed refusal. Mock fixtures only, no
// real Alpaca order.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod mleg_vertical_spread_tests {
    use super::*;
    use crate::mleg::{build_verified_vertical_spread_request, MlegPositionIntent};
    use mqk_execution::option_strategy_permission::{
        OptionLegSide, ProposedOptionLeg, ProposedOptionStrategy,
    };
    use mqk_schemas::{OptionRight, QtyMicros};

    /// A valid call vertical: long $500 call / short $510 call, same
    /// underlying/expiry, opening both legs -- the one shape every test in
    /// this module builds from unless it is deliberately proving a refusal.
    fn proposed_call_vertical() -> ProposedOptionStrategy {
        ProposedOptionStrategy {
            underlying: "AAPL".to_string(),
            legs: vec![
                ProposedOptionLeg {
                    side: OptionLegSide::Long,
                    right: OptionRight::Call,
                    strike_micros: 500_000_000,
                    expiry_yyyymmdd: "20260619".to_string(),
                    qty: QtyMicros::new(1_000_000),
                    multiplier: 100,
                },
                ProposedOptionLeg {
                    side: OptionLegSide::Short,
                    right: OptionRight::Call,
                    strike_micros: 510_000_000,
                    expiry_yyyymmdd: "20260619".to_string(),
                    qty: QtyMicros::new(1_000_000),
                    multiplier: 100,
                },
            ],
            covering_share_qty: None,
            cash_secured_collateral_micros: None,
        }
    }

    fn verified_request() -> VerifiedVerticalSpreadSubmission {
        build_verified_vertical_spread_request(
            &proposed_call_vertical(),
            "AAPL260619C00500000",
            "AAPL260619C00510000",
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
            "test-co-1".to_string(),
            "1".to_string(),
            "2.50".to_string(),
            "day".to_string(),
        )
        .expect("proposed_call_vertical must build a verified submission")
    }

    /// The capability defaults off in `new_for_test` -- refusal must happen
    /// before any HTTP call. `MockServer` is intentionally not started: a
    /// network call here would panic against a non-listening address,
    /// proving this really is a pre-HTTP refusal.
    #[test]
    fn submit_vertical_spread_refuses_before_any_http_call_when_capability_disabled() {
        let adapter = AlpacaBrokerAdapter::new_for_test("http://127.0.0.1:1".to_string());
        let err = adapter
            .submit_vertical_spread(&verified_request())
            .unwrap_err();
        assert_eq!(
            err,
            BrokerError::Reject {
                code: "options_mleg_capability_disabled".to_string(),
                detail: "submit_vertical_spread: options_mleg_capability_enabled is false -- \
                         this capability defaults off and must be explicitly opted into"
                    .to_string(),
            }
        );
    }

    #[test]
    fn submit_vertical_spread_sends_exactly_one_post_with_both_legs() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v2/orders")
                .json_body_partial(r#"{"order_class": "mleg"}"#)
                .json_body_partial(r#"{"legs": [{"symbol": "AAPL260619C00500000"}, {"symbol": "AAPL260619C00510000"}]}"#);
            then.status(200).json_body(serde_json::json!({
                "id": "parent-1",
                "client_order_id": "test-co-1",
                "order_class": "mleg",
                "qty": "1",
                "status": "accepted",
                "legs": [
                    {"id": "leg-long-1", "symbol": "AAPL260619C00500000", "side": "buy", "status": "accepted"},
                    {"id": "leg-short-1", "symbol": "AAPL260619C00510000", "side": "sell", "status": "accepted"}
                ]
            }));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url())
            .with_options_mleg_capability_enabled(true);
        let submitted = adapter
            .submit_vertical_spread(&verified_request())
            .expect("capability enabled, mock returns exactly two legs -- must succeed");

        assert_eq!(submitted.broker_parent_order_id, "parent-1");
        assert_eq!(submitted.long_leg.broker_leg_order_id, "leg-long-1");
        assert_eq!(submitted.short_leg.broker_leg_order_id, "leg-short-1");
        // Exactly one submission for both legs -- never two independent
        // per-leg orders.
        mock.assert_hits(1);
    }

    /// Negative control: a broker response missing a leg must never be
    /// treated as a normal success -- the order may be live at the broker
    /// in an unproven shape, so this must surface as `AmbiguousSubmit`
    /// (fail closed, no synthetic completion), not `Reject`.
    #[test]
    fn submit_vertical_spread_fails_closed_when_broker_returns_wrong_leg_count() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(200).json_body(serde_json::json!({
                "id": "parent-2",
                "client_order_id": "test-co-1",
                "order_class": "mleg",
                "qty": "1",
                "status": "accepted",
                "legs": [
                    {"id": "leg-long-1", "symbol": "AAPL260619C00500000", "side": "buy", "status": "accepted"}
                ]
            }));
        });

        let adapter = AlpacaBrokerAdapter::new_for_test(server.base_url())
            .with_options_mleg_capability_enabled(true);
        let err = adapter
            .submit_vertical_spread(&verified_request())
            .unwrap_err();
        assert!(
            matches!(err, BrokerError::AmbiguousSubmit { .. }),
            "a leg-count mismatch must fail closed as AmbiguousSubmit, got {err:?}"
        );
    }

    #[test]
    fn cancel_vertical_spread_always_refuses_no_http_call() {
        let adapter = AlpacaBrokerAdapter::new_for_test("http://127.0.0.1:1".to_string())
            .with_options_mleg_capability_enabled(true);
        let err = adapter.cancel_vertical_spread("parent-1").unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "options_mleg_cancel_unproven"
        ));
    }

    #[test]
    fn replace_vertical_spread_always_refuses_no_http_call() {
        let adapter = AlpacaBrokerAdapter::new_for_test("http://127.0.0.1:1".to_string())
            .with_options_mleg_capability_enabled(true);
        let err = adapter.replace_vertical_spread("parent-1").unwrap_err();
        assert!(matches!(
            err,
            BrokerError::Reject { code, .. } if code == "options_mleg_replace_unproven"
        ));
    }
}
