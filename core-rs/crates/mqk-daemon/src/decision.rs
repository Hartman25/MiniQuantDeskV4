//! CC-01D: Internal strategy decision-to-intent seam.
//!
//! Provides the narrowest fail-closed path that lets an internally-originated
//! strategy decision be validated against the canonical strategy registry and
//! converted into a durable execution intent candidate via the canonical
//! outbox path.
//!
//! # Gate sequence
//!
//! ```text
//! 0.  field_validation     — decision_id / strategy_id / symbol / side / qty
//! 1.  day_signal_limit     — PT-AUTO-02: per-run intake bound not exceeded (account-wide)
//! 1f. symbol_day_order_cap — MULTI-SYMBOL-DAY-ORDER-CAP-01: optional per-symbol daily order count cap (cap #4)
//! 1e. capital_budget       — B6/TV-04B: per-strategy budget authorized (same gate as external signal path)
//! 1g. per_symbol_notional  — MULTI-SYMBOL-CAPITAL-CAPS-01: optional per-symbol notional cap (cap #3); limit orders only, SizingUnverifiable pass-through for market orders
//! 1h. sector_risk          — ETF-RISK-CLOSURE-01: optional per-sector live gross exposure cap (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`); uses real live weights/marks, fail-closed when enabled and unverifiable, risk-reducing orders always allowed; shared with the external signal path via `capital_policy::sector_risk_gate` (ETF-RISK-EXTERNAL-SIGNAL-GATE-01)
//! 2.  db_present           — no DB → unavailable
//! 3.  registry_check       — strategy must be registered AND enabled
//! 3b. paper_promotion      — STRATEGY-PROMOTION-REGISTRY-01D: exact (strategy_id, symbol, timeframe_secs) identity must be `active_paper`; registered+enabled is necessary but not sufficient; shared with the external signal path via `promotion_gate::evaluate_paper_promotion_gate`
//! 4.  suppression_check    — strategy must not be actively suppressed (per-strategy targeted query)
//! 5.  arm_state            — durable arm state must be ARMED
//! 6.  active_run           — active run must exist and be in "running" state
//! 7.  outbox_enqueue       — durable idempotent write (signal_source = "internal_strategy_decision")
//! ```
//!
//! This is a library function, not an HTTP handler.  Callers receive a
//! structured [`InternalDecisionOutcome`] rather than an HTTP response.
//! The function is intentionally narrow: it does not schedule, allocate, or
//! reason about alpha.

use std::collections::BTreeMap;
use std::sync::Arc;

use mqk_schemas::{QtyMicros, QTY_MICROS_SCALE};
use uuid::Uuid;

use crate::state::AppState;

/// Runtime position book from the execution snapshot: every position is
/// carried exactly as `QtyMicros` -- a fractional position is retained, never
/// dropped or reported as flat.
pub fn position_book_from_snapshot(
    positions: &[mqk_runtime::observability::PositionSnapshot],
) -> BTreeMap<String, QtyMicros> {
    positions
        .iter()
        .map(|p| (p.symbol.clone(), p.net_qty))
        .collect()
}

/// Checked sum of strategy target quantities. `None` on overflow.
pub(crate) fn sum_target_qty<'a>(
    targets: impl IntoIterator<Item = &'a mqk_strategy::TargetPosition>,
) -> Option<QtyMicros> {
    targets
        .into_iter()
        .try_fold(QtyMicros::ZERO, |acc, t| acc.checked_add(t.qty))
}

/// Outcome of the last native-strategy bar dispatch this session.
///
/// `NoDispatch` (no bar dispatched yet) is a different state from every
/// evaluated outcome: a flat, fractional, or overflowed signal is never
/// observable as `NoDispatch`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LastBarSignal {
    NoDispatch,
    /// Exact signed sum of the strategy's target quantities (zero == flat).
    Evaluated(QtyMicros),
    /// `on_bar` ran but the target sum overflowed; no exact quantity exists.
    TotalOverflowed,
}

/// The exact quantity has no whole-unit (V1) representation.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NotRepresentableInV1;

impl LastBarSignal {
    pub fn from_total(total: Option<QtyMicros>) -> Self {
        total.map_or(Self::TotalOverflowed, Self::Evaluated)
    }

    pub fn exact(self) -> Option<QtyMicros> {
        match self {
            Self::Evaluated(q) => Some(q),
            Self::NoDispatch | Self::TotalOverflowed => None,
        }
    }

    /// A bar was dispatched and the strategy provably returned a zero total.
    pub fn is_flat(self) -> bool {
        matches!(self, Self::Evaluated(q) if q.is_zero())
    }

    /// Stable machine label for the V2 surface.
    pub fn state_label(self) -> &'static str {
        match self {
            Self::NoDispatch => "no_dispatch",
            Self::Evaluated(_) => "evaluated",
            Self::TotalOverflowed => "total_overflowed",
        }
    }

    /// V1 whole-unit projection: `Ok(None)` only when nothing was dispatched;
    /// a fractional or overflowed signal is refused, never truncated or
    /// reported as absent.
    pub fn v1_whole_units(self) -> Result<Option<i64>, NotRepresentableInV1> {
        match self {
            Self::NoDispatch => Ok(None),
            Self::Evaluated(q) => q
                .to_whole_units_checked()
                .map(Some)
                .ok_or(NotRepresentableInV1),
            Self::TotalOverflowed => Err(NotRepresentableInV1),
        }
    }
}

/// A signal exists unless the total is provably zero (an overflowed total is
/// not provably zero).
pub(crate) fn signal_generated(total: Option<QtyMicros>) -> bool {
    total.is_none_or(|t| !t.is_zero())
}

/// `order_json["qty"]` encoding understood by the runtime decoder
/// (`mqk_runtime::orchestrator::outbox::parse_signed_qty_micros_field`): a
/// whole quantity is a JSON integer (byte-identical to the historical Equity
/// shape); a fractional quantity is a canonical decimal string. Never a
/// floating-point JSON number.
pub fn order_json_qty_value(qty: QtyMicros) -> serde_json::Value {
    match qty.to_whole_units_checked() {
        Some(units) => serde_json::json!(units),
        None => serde_json::Value::String(qty.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// An internally-originated strategy decision submitted for validation and
/// outbox enqueue.
///
/// All string fields must be non-empty and trimmed by the caller.
#[derive(Debug, Clone)]
pub struct InternalStrategyDecision {
    /// Caller-assigned stable identity for this decision (idempotency key).
    ///
    /// Must be non-empty and unique per logical decision.  Resubmitting the
    /// same `decision_id` is safe: Gate 7 is idempotent (ON CONFLICT DO NOTHING).
    pub decision_id: String,
    /// Authoritative strategy identifier.  Must match a registered + enabled
    /// row in `sys_strategy_registry`.
    pub strategy_id: String,
    /// Ticker symbol (e.g. "AAPL").
    pub symbol: String,
    /// Canonical strategy timeframe in seconds (matches
    /// `StrategySpec::timeframe_secs`).  STRATEGY-PROMOTION-REGISTRY-01D:
    /// part of the exact `(strategy_id, symbol, timeframe_secs)` identity
    /// the paper-promotion gate (Gate 3b) checks — must be positive.
    pub timeframe_secs: i64,
    /// RUNTIME-PROMOTION-EVIDENCE-BINDING-01 (C2): the exact
    /// [`mqk_strategy::Strategy::semantic_fingerprint`] of the host instance
    /// that produced this decision (`StrategyBarResult::semantic_fingerprint`,
    /// captured by `StrategyHost::on_bar` from the boxed instance it holds --
    /// never re-derived from ambient environment state after the fact). The
    /// paper-promotion gate (Gate 3b) refuses this decision if it does not
    /// exactly match the durable `active_paper` config_fingerprint. Callers
    /// that construct an `InternalStrategyDecision` outside
    /// `bar_result_to_decisions` (test fixtures) must supply the same value
    /// the promotion gate is expected to accept.
    pub strategy_semantic_fingerprint: String,
    /// Order side: "buy" or "sell" (case-insensitive; normalised internally).
    pub side: String,
    /// Actual asset quantity (`1` share == `QTY_MICROS_SCALE` raw).  Must be positive.
    pub qty: QtyMicros,
    /// Order type: "market" or "limit".
    pub order_type: String,
    /// Time-in-force: "day", "gtc", "ioc", "fok".
    pub time_in_force: String,
    /// Limit price in cents (required when order_type == "limit").
    pub limit_price: Option<i64>,
}

/// Outcome of a single call to [`submit_internal_strategy_decision`].
#[derive(Debug, Clone)]
pub struct InternalDecisionOutcome {
    /// `true` only when Gate 7 returned `Ok(true)` (new outbox row inserted).
    /// `false` for duplicates and all gate failures.
    pub accepted: bool,
    /// Machine-readable disposition:
    ///
    /// | value              | meaning                                              |
    /// |--------------------|------------------------------------------------------|
    /// | `"accepted"`       | passed all gates; new outbox row inserted            |
    /// | `"duplicate"`      | decision_id already in outbox; no new row            |
    /// | `"rejected"`       | field validation failure, registry gate failure, or per-symbol notional cap denial (cap #3, Gate 1g) |
    /// | `"unavailable"`    | transient system state (no DB, arm-state I/O, run)   |
    /// | `"suppressed"`     | strategy is actively suppressed                      |
    /// | `"day_limit_reached"` | PT-AUTO-02 per-run intake bound exceeded          |
    /// | `"symbol_day_limit_reached"` | MULTI-SYMBOL-DAY-ORDER-CAP-01: per-symbol daily order count cap exceeded (cap #4, Gate 1f) |
    /// | `"budget_denied"`  | B6/TV-04B: capital policy present but strategy not budget-authorized |
    /// | `"policy_invalid"` | B6/TV-04B: capital policy configured but structurally invalid        |
    /// | `"sector_config_invalid"` | ETF-RISK-CLOSURE-01: `MQK_SECTOR_EXPOSURE_LIMITS_BPS` is set but malformed (Gate 1h) |
    /// | `"sector_weights_missing"` | ETF-RISK-CLOSURE-01: sector risk is enabled for this symbol's sector but live weights/marks could not be established (Gate 1h) |
    /// | `"sector_nav_unavailable"` | ETF-RISK-CLOSURE-01: sector risk is enabled but portfolio NAV is not positive (Gate 1h) |
    /// | `"sector_limit_exceeded"`  | ETF-RISK-CLOSURE-01: candidate order would exceed a configured per-sector gross exposure cap and is not risk-reducing (Gate 1h) |
    /// | `"promotion_missing"` | STRATEGY-PROMOTION-REGISTRY-01D: no promotion record exists for this exact identity (Gate 3b) |
    /// | `"promotion_shadow_only"` | Gate 3b: current state is `shadow_approved` (research/shadow only, never paper-tradable) |
    /// | `"promotion_not_active"` | Gate 3b: current state is `paper_approved` (evidence accepted, activation still required) |
    /// | `"promotion_demoted"` / `"promotion_retired"` / `"promotion_rejected"` / `"promotion_expired"` | Gate 3b: current state blocks trading |
    /// | `"promotion_config_mismatch"` | RUNTIME-PROMOTION-EVIDENCE-BINDING-01: durable `active_paper` exists but this decision's actual strategy semantic config does not match the promoted fingerprint (legacy/unavailable promoted identity, or genuine drift) |
    pub disposition: String,
    /// Echoed from [`InternalStrategyDecision::decision_id`].
    pub decision_id: String,
    /// Echoed from [`InternalStrategyDecision::strategy_id`].
    pub strategy_id: String,
    /// Active run UUID at time of processing (present from Gate 6 onwards).
    pub active_run_id: Option<Uuid>,
    /// Human-readable explanations for non-accepted outcomes.  Empty on success.
    pub blockers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Implementation helpers
// ---------------------------------------------------------------------------

fn outcome(
    accepted: bool,
    disposition: &str,
    decision_id: &str,
    strategy_id: &str,
    active_run_id: Option<Uuid>,
    blockers: Vec<String>,
) -> InternalDecisionOutcome {
    InternalDecisionOutcome {
        accepted,
        disposition: disposition.to_string(),
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        active_run_id,
        blockers,
    }
}

// ---------------------------------------------------------------------------
// Gate 0: field validation
// ---------------------------------------------------------------------------

/// Returns `Err(blockers)` if any required field is invalid.
fn validate_fields(d: &InternalStrategyDecision) -> Result<(), Vec<String>> {
    let mut blockers = Vec::new();

    if d.decision_id.trim().is_empty() {
        blockers.push("decision_id must not be blank".to_string());
    }
    if d.strategy_id.trim().is_empty() {
        blockers.push("strategy_id must not be blank".to_string());
    }
    if d.symbol.trim().is_empty() {
        blockers.push("symbol must not be blank".to_string());
    }
    if d.timeframe_secs <= 0 {
        blockers.push("timeframe_secs must be positive".to_string());
    }

    let side = d.side.trim().to_ascii_lowercase();
    if !matches!(side.as_str(), "buy" | "sell") {
        blockers.push("side must be one of: buy, sell".to_string());
    }

    if !d.qty.is_positive() {
        blockers.push("qty must be positive".to_string());
    } else if d.qty.raw() > (i32::MAX as i64) * QTY_MICROS_SCALE {
        blockers.push("qty is out of range for broker request".to_string());
    }

    let order_type = d.order_type.trim().to_ascii_lowercase();
    if !matches!(order_type.as_str(), "market" | "limit") {
        blockers.push("order_type must be one of: market, limit".to_string());
    }

    let tif = d.time_in_force.trim().to_ascii_lowercase();
    if !matches!(tif.as_str(), "day" | "gtc" | "ioc" | "fok") {
        blockers.push("time_in_force must be one of: day, gtc, ioc, fok".to_string());
    }

    if order_type == "limit" && d.limit_price.is_none() {
        blockers.push("limit_price is required when order_type is 'limit'".to_string());
    }

    if blockers.is_empty() {
        Ok(())
    } else {
        Err(blockers)
    }
}

// ---------------------------------------------------------------------------
// order_json shape for the outbox
// ---------------------------------------------------------------------------

/// Currency of the account every order, cap and equity figure is denominated
/// in. The single account-currency seam in the daemon: callers compare an
/// instrument's quote currency against it and never against a literal. There
/// is no FX conversion authority yet, so an order on an instrument quoted in
/// any other currency is refused (fail closed) rather than valued as if it
/// were this one; an account- or venue-derived currency replaces this value
/// here without touching the comparisons.
pub(crate) const ACCOUNT_CURRENCY: &str = "USD";

#[derive(Debug, Clone)]
struct DurableOrderInstrumentContext {
    asset_class: String,
    economics_snapshot: Option<serde_json::Value>,
}

impl DurableOrderInstrumentContext {
    fn legacy_equity() -> Self {
        Self {
            asset_class: "equity".to_string(),
            economics_snapshot: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum OrderInstrumentContextError {
    Unavailable(String),
    Rejected(String),
}

fn resolve_order_instrument_context_from_registry(
    registry: &mqk_md::instrument_registry_v2::InstrumentRegistryV2,
    deployment_mode: crate::state::DeploymentMode,
    broker_kind: Option<crate::state::BrokerKind>,
    symbol: &str,
    legacy_equity_allowed: bool,
) -> Result<DurableOrderInstrumentContext, OrderInstrumentContextError> {
    mqk_md::instrument_registry_v2::validate_registry_v2(registry).map_err(|err| {
        OrderInstrumentContextError::Unavailable(format!(
            "trading registry-v2 validation failed: {err}"
        ))
    })?;

    if registry
        .instruments
        .iter()
        .any(|instrument| instrument.allow_enabled_non_equity_for_testing)
    {
        return Err(OrderInstrumentContextError::Unavailable(
            "trading registry-v2 contains              allow_enabled_non_equity_for_testing=true;              test-only validator bypasses are forbidden in production              trading authority"
                .to_string(),
        ));
    }

    let symbol = symbol.trim();

    let Some(instrument) = registry
        .instruments
        .iter()
        .find(|instrument| instrument.symbol.trim() == symbol)
    else {
        if legacy_equity_allowed {
            return Ok(DurableOrderInstrumentContext::legacy_equity());
        }

        return Err(OrderInstrumentContextError::Rejected(format!(
            "symbol '{}' is neither present in the configured trading              registry-v2 source nor an enabled canonical legacy Equity",
            symbol
        )));
    };

    match instrument.asset_class.trim() {
        "equity" if legacy_equity_allowed => {
            Ok(DurableOrderInstrumentContext::legacy_equity())
        }

        "equity" => Err(OrderInstrumentContextError::Rejected(format!(
            "registry-v2 symbol '{}' is tagged Equity but is not enabled in              the canonical legacy Equity registry",
            instrument.symbol
        ))),

        "crypto" => {
            if deployment_mode != crate::state::DeploymentMode::Paper {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' is registry-v2 trading configured \
                     but M6 permits this production cutover only in Paper mode",
                    instrument.symbol
                )));
            }

            if broker_kind
                != Some(crate::state::BrokerKind::Alpaca)
            {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' requires the configured broker                      to be Alpaca before Alpaca broker provenance may be                      persisted",
                    instrument.symbol
                )));
            }

            if !instrument.paper_trading_enabled {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' is not paper_trading_enabled in \
                     the trading registry-v2 source",
                    instrument.symbol
                )));
            }

            // B1 must never create Live authority as a side effect of Paper
            // enablement.
            if instrument.live_trading_enabled {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' has live_trading_enabled=true; \
                     B1 Paper cutover refuses Live-enabled registry rows",
                    instrument.symbol
                )));
            }

            let broker_symbol = instrument
                .broker_symbols
                .get("alpaca")
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OrderInstrumentContextError::Rejected(format!(
                        "crypto instrument '{}' has no Alpaca broker symbol",
                        instrument.symbol
                    ))
                })?;

            // B1 BTC/USD uses the same canonical and Alpaca wire symbol.
            // Do not silently invent a translation mechanism here.
            if broker_symbol != instrument.symbol.trim() {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' maps to Alpaca symbol '{}'; \
                     B1 requires exact canonical/broker symbol identity",
                    instrument.symbol,
                    broker_symbol
                )));
            }

            let session_profile = instrument
                .economics
                .as_ref()
                .and_then(|economics| economics.session_profile.as_deref())
                .ok_or_else(|| {
                    OrderInstrumentContextError::Rejected(format!(
                        "crypto instrument '{}' has no economics.session_profile",
                        instrument.symbol
                    ))
                })?;

            if session_profile
                != mqk_md::instrument_registry_v2::SESSION_PROFILE_CRYPTO_24_7
            {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' session_profile '{}' is not '{}'",
                    instrument.symbol,
                    session_profile,
                    mqk_md::instrument_registry_v2::SESSION_PROFILE_CRYPTO_24_7
                )));
            }

            let bridged =
                crate::state::instrument_economics_bridge::instrument_v2_to_economics(
                    instrument,
                );

            let economics = bridged.economics.ok_or_else(|| {
                OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' could not bridge to order economics: \
                     truth_state={} reason_code={}",
                    instrument.symbol,
                    bridged.truth_state,
                    bridged.reason_code
                ))
            })?;

            // No FX authority exists: sizing, allocation caps and account
            // equity are all denominated in the account currency, so an
            // instrument quoted in another currency would be valued as if it
            // were that currency. Refuse instead of mixing currencies.
            if economics.quote_currency != ACCOUNT_CURRENCY {
                return Err(OrderInstrumentContextError::Rejected(format!(
                    "crypto instrument '{}' is quoted in '{}' but the account currency is '{}'; \
                     no FX conversion authority exists (currency_conversion_unsupported)",
                    instrument.symbol, economics.quote_currency, ACCOUNT_CURRENCY
                )));
            }

            let min_trade_qty_micros =
                economics.min_trade_qty_micros.ok_or_else(|| {
                    OrderInstrumentContextError::Rejected(format!(
                        "crypto instrument '{}' is missing min_trade_qty_micros",
                        instrument.symbol
                    ))
                })?;

            let tick_size_micros =
                economics.tick_size_micros.ok_or_else(|| {
                    OrderInstrumentContextError::Rejected(format!(
                        "crypto instrument '{}' is missing tick_size_micros",
                        instrument.symbol
                    ))
                })?;

            let quantity_increment_micros =
                economics.quantity_increment_micros.ok_or_else(|| {
                    OrderInstrumentContextError::Rejected(format!(
                        "crypto instrument '{}' is missing \
                         quantity_increment_micros",
                        instrument.symbol
                    ))
                })?;

            let snapshot = serde_json::json!({
                "source": "registry_v2",
                "authority": "MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH",
                "registry_schema_version": registry.schema_version,
                "instrument_id": economics.instrument_id,
                "symbol": economics.symbol,
                "asset_class": economics.asset_class,
                "quote_currency": economics.quote_currency,
                "contract_multiplier_micros":
                    economics.contract_multiplier_micros,
                "quantity_scale": economics.quantity_scale,
                "min_trade_qty_micros": min_trade_qty_micros,
                "tick_size_micros": tick_size_micros,
                "quantity_increment_micros":
                    quantity_increment_micros,
                "session_profile": session_profile,
                "broker": "alpaca",
                "broker_symbol": broker_symbol,
            });

            Ok(DurableOrderInstrumentContext {
                asset_class: "crypto".to_string(),
                economics_snapshot: Some(snapshot),
            })
        }

        other => Err(OrderInstrumentContextError::Rejected(format!(
            "instrument '{}' resolved from the trading registry-v2 source \
             with unsupported B1 asset_class '{}'",
            instrument.symbol,
            other
        ))),
    }
}

/// The canonical positive proof that `symbol` is an enabled Equity: the v1
/// instrument registry must list it. `Err` means the authority could not be
/// read/validated (unavailable), which is never the same as "not an Equity".
pub(crate) fn legacy_equity_symbol_is_enabled(
    state: &AppState,
    symbol: &str,
) -> Result<bool, OrderInstrumentContextError> {
    let instruments = mqk_md::instrument_registry::load_instrument_registry(std::path::Path::new(
        &state.instrument_registry_path,
    ))
    .map_err(|err| {
        OrderInstrumentContextError::Unavailable(format!(
            "canonical legacy Equity registry load failed from '{}': {err}",
            state.instrument_registry_path
        ))
    })?;

    mqk_md::instrument_registry::validate_registry(&instruments).map_err(|err| {
        OrderInstrumentContextError::Unavailable(format!(
            "canonical legacy Equity registry validation failed: {err}"
        ))
    })?;

    let symbol = symbol.trim();

    Ok(mqk_md::instrument_registry::enabled_equities(&instruments)
        .into_iter()
        .any(|instrument| instrument.symbol.trim() == symbol))
}

fn resolve_order_instrument_context(
    state: &AppState,
    symbol: &str,
) -> Result<DurableOrderInstrumentContext, OrderInstrumentContextError> {
    let trading_registry_path = state
        .trading_instrument_registry_v2_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty());

    // Even without a v2 trading source, legacy Equity authority is explicit:
    // the canonical v1 registry must independently prove this symbol enabled.
    let Some(path) = trading_registry_path else {
        if legacy_equity_symbol_is_enabled(state, symbol)? {
            return Ok(DurableOrderInstrumentContext::legacy_equity());
        }

        return Err(OrderInstrumentContextError::Rejected(format!(
            "symbol '{}' is not an enabled canonical legacy Equity and no \
             trading registry-v2 authority is configured",
            symbol.trim()
        )));
    };

    let registry =
        mqk_md::instrument_registry_v2::load_instrument_registry_v2(std::path::Path::new(path))
            .map_err(|err| {
                OrderInstrumentContextError::Unavailable(format!(
                    "trading registry-v2 load failed from '{}': {err}",
                    path
                ))
            })?;

    let matching = registry
        .instruments
        .iter()
        .find(|instrument| instrument.symbol.trim() == symbol.trim());

    // Only an absent or explicitly Equity v2 row may fall back to v1,
    // and v1 must prove that Equity authority independently.
    let legacy_equity_allowed = match matching {
        None => legacy_equity_symbol_is_enabled(state, symbol)?,

        Some(instrument) if instrument.asset_class.trim() == "equity" => {
            legacy_equity_symbol_is_enabled(state, symbol)?
        }

        Some(_) => false,
    };

    resolve_order_instrument_context_from_registry(
        &registry,
        state.deployment_mode(),
        state.runtime_selection().broker_kind,
        symbol,
        legacy_equity_allowed,
    )
}

/// Identity proof for an order surface that carries no asset class (the
/// manual operator order): the symbol must resolve, through the same registry
/// authority the internal decision seam uses, to an Equity. A symbol that
/// resolves to anything else, or that no registry proves, is refused rather
/// than written to the outbox as an implied Equity.
pub(crate) fn prove_equity_order_identity(
    state: &AppState,
    symbol: &str,
) -> Result<(), OrderInstrumentContextError> {
    let context = resolve_order_instrument_context(state, symbol)?;
    if context.asset_class == "equity" {
        return Ok(());
    }
    Err(OrderInstrumentContextError::Rejected(format!(
        "symbol '{}' resolves to asset class '{}' and this order surface carries no asset class: \
         only a registry-proven Equity is accepted here",
        symbol.trim(),
        context.asset_class
    )))
}

/// Why a flatten close for `symbol` cannot be an Equity close, or `None`.
///
/// A flatten close is an Equity-shaped order on the Equity domain's run (no
/// `asset_class`, `day` time-in-force, whole shares). That is wrong for any
/// instrument the trading registry-v2 positively lists as non-Equity, so such
/// a position is refused with a reason instead of being enqueued as a
/// mislabelled Equity close that the dispatch parser quarantines (fractional
/// quantity) or the broker rejects. A symbol the registry does not list, or no
/// readable registry-v2, keeps the historical close, so liquidating an
/// ordinary Equity position never depends on this registry being available.
pub fn flatten_refusal_for_symbol(state: &AppState, symbol: &str) -> Option<String> {
    let path = state
        .trading_instrument_registry_v2_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    let registry =
        mqk_md::instrument_registry_v2::load_instrument_registry_v2(std::path::Path::new(path))
            .ok()?;
    let symbol = symbol.trim();
    let instrument = registry
        .instruments
        .iter()
        .find(|instrument| instrument.symbol.trim() == symbol)?;
    let class = instrument.asset_class.trim();
    (class != "equity").then(|| {
        format!(
            "position '{symbol}' is a '{class}' instrument; the flatten close path addresses \
             the Equity domain run and builds an Equity-shaped order, so it is refused rather \
             than enqueued as a mislabelled Equity close"
        )
    })
}

/// The execution domain that owns an order, from its registry-resolved asset
/// class. Only a positively resolved Crypto instrument belongs to
/// `Crypto24_7`; every other outcome (Equity, or an unresolved/refused symbol
/// that Gate 7 refuses anyway) stays with `EquityNyse`, the historical owner
/// of this seam. A future asset class or venue is admitted only by adding its
/// resolver arm in Gate 7 and its domain here; until then it is refused, never
/// routed to a domain by default.
fn execution_domain_for_resolved_context(
    resolved: &Result<DurableOrderInstrumentContext, OrderInstrumentContextError>,
) -> crate::state::ExecutionDomain {
    match resolved {
        Ok(context) if context.asset_class == "crypto" => crate::state::ExecutionDomain::Crypto24_7,
        _ => crate::state::ExecutionDomain::EquityNyse,
    }
}

/// Non-Equity admission guard: a Crypto order is only ever admitted against an
/// explicit, exactly-parsed operator size. Missing/blank/malformed size
/// inputs fail closed (never a default of one unit), and a decision larger
/// than the explicit target is refused -- e.g. a strategy that silently fell
/// back to its whole-share default. Equity is unaffected.
///
/// Pure: the raw operator inputs are parameters (the caller reads env).
pub(crate) fn non_equity_explicit_size_gate(
    asset_class: &str,
    decision_qty: QtyMicros,
    raw_target: Option<&str>,
    raw_max_target: Option<&str>,
    raw_max_notional_usd: Option<&str>,
) -> Result<(), String> {
    use mqk_execution::AssetClass;
    let class = match asset_class {
        "equity" => return Ok(()),
        "crypto" => AssetClass::Crypto,
        other => {
            return Err(format!(
                "internal decision refused: asset_class '{other}' has no supported \
                 explicit target-sizing policy"
            ))
        }
    };
    let sizing = mqk_strategy::TargetSizing::resolve(
        class,
        raw_target,
        raw_max_target,
        raw_max_notional_usd,
    )
    .map_err(|e| {
        format!(
            "internal decision refused: crypto order requires an explicit exact size \
             (MQK_STRATEGY_TARGET_QTY): {e}"
        )
    })?;
    let limit = sizing
        .max_target_qty()
        .map_or(sizing.target_qty(), |m| m.min(sizing.target_qty()));
    if decision_qty > limit {
        return Err(format!(
            "internal decision refused: crypto order qty {decision_qty} exceeds the explicit \
             configured size {limit}; no default or fallback quantity is ever admitted"
        ));
    }
    Ok(())
}

/// Validate-only time-in-force admission for an order of `asset_class`.
///
/// Crypto time-in-force is EXPLICIT-ONLY (operator policy): the decision must
/// already carry `gtc` or `ioc`, emitted upstream by the strategy/deployment
/// construction seam (`crypto_execution_policy::strategy_time_in_force`). This
/// function never reads policy or environment and never rewrites a value --
/// `day`, `fok`, `opg`, `cls`, empty and unknown are refused. Equity
/// time-in-force is untouched.
fn admit_time_in_force(asset_class: &str, tif: &str) -> Result<String, String> {
    let tif = tif.trim().to_ascii_lowercase();
    if asset_class != "crypto" {
        return Ok(tif);
    }
    match tif.as_str() {
        "gtc" | "ioc" => Ok(tif),
        other => Err(format!(
            "internal decision refused: crypto order time_in_force '{other}' is not admissible \
             (crypto requires an explicit gtc or ioc; no time_in_force is defaulted or rewritten)"
        )),
    }
}

fn build_order_json(
    d: &InternalStrategyDecision,
    instrument: &DurableOrderInstrumentContext,
) -> serde_json::Value {
    let mut order = serde_json::json!({
        "symbol":         d.symbol.trim(),
        "side":           d.side.trim().to_ascii_lowercase(),
        "qty":            order_json_qty_value(d.qty),
        "order_type":     d.order_type.trim().to_ascii_lowercase(),
        "time_in_force":  d.time_in_force.trim().to_ascii_lowercase(),
        "limit_price":    d.limit_price,
        "strategy_id":    d.strategy_id.trim(),
        "strategy_semantic_fingerprint":
            d.strategy_semantic_fingerprint.trim(),
        "timeframe_secs": d.timeframe_secs,
        "signal_source":  "internal_strategy_decision",
    });

    if instrument.asset_class != "equity" {
        order["asset_class"] = serde_json::Value::String(instrument.asset_class.clone());
    }

    // B5: persist the canonical identity of the policy behind this order's
    // time_in_force, derived purely from the value the construction seam
    // already emitted -- audit never depends on a mutable environment var.
    if instrument.asset_class == "crypto" {
        if let Some(fp) =
            crate::state::crypto_execution_policy::crypto_execution_policy_fingerprint_for_decision_tif(
                &d.time_in_force,
            )
        {
            order["crypto_execution_policy_fingerprint"] = serde_json::Value::String(fp);
        }
    }

    if let Some(snapshot) = instrument.economics_snapshot.as_ref() {
        order["instrument_economics"] = snapshot.clone();
    }

    order
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// B1C: StrategyBarResult → InternalStrategyDecision translation
// ---------------------------------------------------------------------------

/// B1C: Translate a `StrategyBarResult` from the execution loop into a list of
/// `InternalStrategyDecision`s ready for submission through
/// [`submit_internal_strategy_decision`].
///
/// # Semantics: target position → order delta
///
/// `TargetPosition.qty` is a **signed target portfolio state**, not an
/// incremental order size.  The order qty is the delta between the target and
/// the current held position:
///
/// ```text
/// delta = target.qty - current_positions[symbol]   (0 if symbol absent = flat)
/// delta > 0  →  buy  abs(delta) shares
/// delta < 0  →  sell abs(delta) shares  (only if holdings cover the sell; see B5 guard)
/// delta == 0 →  skip (already at target; no order)
/// ```
///
/// Callers must pass an authoritative `current_positions` map derived from the
/// most recent execution snapshot.  A symbol absent from the map is treated as
/// flat (qty = 0) — correct for symbols with no open position.
///
/// # Fail-closed rules
///
/// - `result.intents.should_execute()` is `false` (shadow mode) → returns empty.
/// - `result.intents.output.targets` is empty → returns empty (no-op bar).
/// - Delta == 0 for a target → skipped (already at target; no order needed).
/// - **B5 short-sale guard**: `delta < 0` AND `current <= 0` → skipped (no long
///   position to sell against; would open a short, which the native strategy
///   runtime does not support).
/// - **B5 short-sale guard**: `delta < 0` AND `abs(delta) > current` → skipped
///   (sell would exceed existing long holdings, driving the position net-short;
///   not supported by this runtime).
///
/// # B5 rationale
///
/// The native strategy runtime tracks portfolio positions but does not manage
/// short-position lifecycle (margin, borrow, cover semantics).  A sell decision
/// that would result in a net-short position is silently dropped here rather
/// than forwarded to the broker where it would either be rejected (causing
/// visible broker error) or filled (resulting in a short position the runtime
/// cannot safely manage).  Fail-closed: skip the unsupported intent rather than
/// propagate it.
///
/// # Output fields
///
/// | Source                | Decision field      | Value                    |
/// |-----------------------|---------------------|--------------------------|
/// | `target.symbol`       | `symbol`            | as-is                    |
/// | `delta > 0`           | `side`              | `"buy"`                  |
/// | `delta < 0`           | `side`              | `"sell"`                 |
/// | `abs(delta)`          | `qty`               | positive share count     |
/// | —                     | `order_type`        | `"market"`               |
/// | —                     | `time_in_force`     | `"day"`                  |
/// | —                     | `limit_price`       | `None`                   |
///
/// `decision_id` is a UUIDv5 derived from
/// `"mqk.strategy-decision.v3|{run_id}|{strategy_id}|{symbol}|{timeframe_secs}|{target_qty}|{bar_end_ts}"`
/// where `target_qty` is the strategy's raw *signed target position*
/// (`TargetPosition::qty`, before subtracting `current`) and `bar_end_ts` is
/// the exact completed-bar identity (`EvaluatedBarFacts::bar_end_ts`) this
/// decision was evaluated against
/// (STRATEGY-DECISION-IDEMPOTENCY-01/STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02).
///
/// This parameter is named `bar_end_ts`, not `now_micros`, deliberately:
/// the 1-second execution loop tick may re-run the same strategy evaluation
/// against the same still-current completed bar many times before a new bar
/// closes (e.g. while an earlier decision for that bar has not yet resolved
/// to a terminal broker outcome). Seeding this identity from wall-clock time
/// (the prior behavior) made every such re-evaluation produce a distinct
/// `decision_id`, defeating the outbox's `ON CONFLICT DO NOTHING` dedup
/// entirely — a real duplicate-live-order path, bounded only by
/// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`, not by design.
///
/// # -02: identity is the target, not the derived delta
///
/// -01 anchored identity on `bar_end_ts` but still included the *derived*
/// order quantity (`delta = target.qty - current`) and `side`. That
/// reintroduces the same class of bug one level down: `current` is read
/// from the live portfolio snapshot, which moves the instant the strategy's
/// own already-working order for this exact bar/target receives a partial
/// fill — so re-evaluating the *same* bar/target after a partial fill
/// computed a *different* `delta`, and therefore a *different*
/// `decision_id`, for what is economically the identical intent. That let a
/// second order reach the outbox for the remaining (post-partial-fill)
/// delta while the original order's remaining quantity was still working —
/// two live orders racing to fill the same target.
///
/// Anchoring on `target_qty` instead (the strategy's target, which by
/// construction does not move when a fill against the current in-flight
/// order updates `current`) means the same logical intent — same run,
/// strategy, symbol, timeframe, completed bar, target — always produces the
/// same `decision_id` regardless of how `current`/`delta` drift while that
/// intent's order is still working. Because `decision_id` is also the
/// outbox `idempotency_key` (Gate 7, `ON CONFLICT (idempotency_key) DO
/// NOTHING`), this makes "at most one durable order per intent" a property
/// the database enforces directly, not something every caller must
/// separately reason about — the *first* delta computed for an intent is
/// the one, and only one, that is ever durably submitted; a later
/// re-evaluation computing a smaller delta (because part of the first order
/// already filled) is rejected as a duplicate before it can add exposure
/// beyond the original target. See `runtime_opportunity_allocation.rs`'s
/// `compute_cycle_id` for the same bar-anchored-identity pattern applied to
/// the allocator's economic-cycle identity.
///
/// This function is pure (no IO, no state mutation) and exported for test
/// isolation.
pub fn bar_result_to_decisions(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_end_ts: i64,
    current_positions: &BTreeMap<String, QtyMicros>,
) -> Vec<InternalStrategyDecision> {
    bar_result_to_decisions_with_tif(result, run_id, bar_end_ts, current_positions, |_| {
        Ok("day".to_string())
    })
}

/// Construction seam: like [`bar_result_to_decisions`], but the
/// time-in-force of each decision is supplied by the strategy/deployment
/// policy (`tif_for_symbol`) instead of the generic `"day"`. An `Err` refuses
/// that decision before it is constructed. A non-`day` value participates in
/// `decision_id` (a `gtc`<->`ioc` change is a different economic intent);
/// `day` reproduces the historical identity byte-for-byte.
pub fn bar_result_to_decisions_with_tif<F>(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_end_ts: i64,
    current_positions: &BTreeMap<String, QtyMicros>,
    tif_for_symbol: F,
) -> Vec<InternalStrategyDecision>
where
    F: Fn(&str) -> Result<String, String>,
{
    if !result.intents.should_execute() {
        return vec![];
    }
    let strategy_id = result.spec.name.clone();
    result
        .intents
        .output
        .targets
        .iter()
        .filter_map(|t| {
            // Delta-to-target: TargetPosition.qty is a target portfolio state,
            // not an incremental order size.  Symbols absent from the map are
            // treated as flat (current = 0).
            let current = current_positions
                .get(&t.symbol)
                .copied()
                .unwrap_or(QtyMicros::ZERO);
            let Some(delta) = t.qty.checked_sub(current) else {
                tracing::error!(
                    symbol = %t.symbol,
                    target_qty = %t.qty,
                    current_qty = %current,
                    "target_minus_current_overflow: refusing decision"
                );
                return None;
            };
            if delta.is_zero() {
                return None; // already at target; no order needed
            }
            // SHORT-SIDE-INTENT-MODEL-01: classify intent explicitly so the
            // control plane distinguishes sell-to-close from short-open.
            // Short-open and sell-beyond-long are blocked fail-closed (B5 backstop).
            // Call evaluate_short_entry_policy directly for policy diagnostics; see
            // scenario_short_side_intent_model_01 for integrated proof tests.
            // Classification is scale-invariant (pure comparisons between
            // same-unit values), so raw micros classify exactly like units.
            let intent = crate::capital_policy::classify_order_intent(current.raw(), delta.raw());
            let (side, qty) = match intent {
                crate::capital_policy::OrderIntent::LongOpen
                | crate::capital_policy::OrderIntent::BuyToCover
                | crate::capital_policy::OrderIntent::BuyToFlat
                | crate::capital_policy::OrderIntent::BuyBeyondShortToLong => {
                    ("buy".to_string(), delta)
                }
                crate::capital_policy::OrderIntent::SellToClose
                | crate::capital_policy::OrderIntent::SellToFlat => {
                    ("sell".to_string(), delta.checked_neg()?)
                }
                crate::capital_policy::OrderIntent::ShortOpen
                | crate::capital_policy::OrderIntent::SellBeyondLongToShort
                | crate::capital_policy::OrderIntent::NoOp => return None,
            };
            let time_in_force = match tif_for_symbol(&t.symbol) {
                Ok(tif) => tif,
                Err(reason) => {
                    tracing::error!(
                        symbol = %t.symbol,
                        reason = %reason,
                        "time_in_force_policy_refused: no decision constructed"
                    );
                    return None;
                }
            };
            // STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02: identity is anchored
            // on the strategy's raw signed TARGET (`t.qty`), never on the
            // derived `delta`/`side` — see the doc comment above for why.
            let mut identity = format!(
                "mqk.strategy-decision.v3|{run_id}|{strategy_id}|{symbol}|{timeframe_secs}|{target_qty}|{bar_end_ts}",
                symbol = t.symbol,
                timeframe_secs = result.spec.timeframe_secs,
                target_qty = t.qty,
            );
            if time_in_force != "day" {
                identity.push_str(&format!("|tif={time_in_force}"));
            }
            let decision_id =
                Uuid::new_v5(&Uuid::NAMESPACE_DNS, identity.as_bytes()).to_string();
            Some(InternalStrategyDecision {
                decision_id,
                strategy_id: strategy_id.clone(),
                symbol: t.symbol.clone(),
                timeframe_secs: result.spec.timeframe_secs,
                strategy_semantic_fingerprint: result.semantic_fingerprint.clone(),
                side,
                qty,
                order_type: "market".to_string(),
                time_in_force,
                limit_price: None,
            })
        })
        .collect()
}

/// STRATEGY-DECISION-IDEMPOTENCY-01: the one production seam that decides
/// whether a bar evaluation's decisions are safe to compute at all, given
/// whatever completed-bar-identity evidence (`EvaluatedBarFacts`) the
/// dispatch pipeline was able to establish for this tick.
///
/// `bar_facts.is_none()` means the legacy stub-context fallback fired
/// (`AppState::dispatch_native_strategy_for_symbol_with_bar_and_facts`'s
/// no-DB-context branch, which always yields `is_complete=false` and
/// therefore empty `targets` in practice) -- there is no durably provable
/// bar identity to anchor `decision_id` on. Rather than fall back to
/// wall-clock time (which would silently reintroduce the exact duplicate-
/// decision-id bug STRATEGY-DECISION-IDEMPOTENCY-01 closes), this refuses
/// to produce any decision at all when facts are missing and the strategy
/// nonetheless produced a nonzero target -- a structural fail-closed
/// backstop, not the expected path.
pub fn decisions_from_bar_facts(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_facts: Option<&crate::state::EvaluatedBarFacts>,
    current_positions: &BTreeMap<String, QtyMicros>,
) -> Vec<InternalStrategyDecision> {
    match bar_facts {
        Some(facts) => bar_result_to_decisions(result, run_id, facts.bar_end_ts, current_positions),
        None => {
            if !result.intents.output.targets.is_empty() {
                tracing::error!(
                    run_id = %run_id,
                    "decision_id_bar_anchor_missing: strategy produced target(s) with no \
                     provable completed-bar identity (bar_facts unavailable); refusing to \
                     submit any decision this tick"
                );
            }
            Vec::new()
        }
    }
}

/// Production variant of [`decisions_from_bar_facts`]: identical bar-identity
/// fail-closed handling, with decision time-in-force supplied by the
/// strategy/deployment policy via [`bar_result_to_decisions_with_tif`].
pub fn decisions_from_bar_facts_with_tif<F>(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_facts: Option<&crate::state::EvaluatedBarFacts>,
    current_positions: &BTreeMap<String, QtyMicros>,
    tif_for_symbol: F,
) -> Vec<InternalStrategyDecision>
where
    F: Fn(&str) -> Result<String, String>,
{
    match bar_facts {
        Some(facts) => bar_result_to_decisions_with_tif(
            result,
            run_id,
            facts.bar_end_ts,
            current_positions,
            tif_for_symbol,
        ),
        None => decisions_from_bar_facts(result, run_id, None, current_positions),
    }
}

/// Registry-resolved asset class of `symbol` (`"equity"`, `"crypto"`), used by
/// the construction seam to pick the class's time-in-force. Resolution only:
/// no policy is applied here.
pub(crate) fn resolve_symbol_asset_class(state: &AppState, symbol: &str) -> Result<String, String> {
    match resolve_order_instrument_context(state, symbol) {
        Ok(ctx) => Ok(ctx.asset_class),
        Err(OrderInstrumentContextError::Unavailable(m))
        | Err(OrderInstrumentContextError::Rejected(m)) => Err(m),
    }
}

/// Validate an internally-originated strategy decision against the canonical
/// registry and enqueue it to the durable outbox path.
///
/// The gate sequence is strictly ordered (fail-fast).  See module docs for
/// the full sequence and disposition values.
///
/// This function is `async` because Gates 2–7 require DB and state reads.
/// It does NOT hold the lifecycle mutex lock for its entire duration —
/// lifecycle_guard is not acquired here because this is not an HTTP handler
/// and its callers are expected to manage concurrency at a higher level.
pub async fn submit_internal_strategy_decision(
    state: &Arc<AppState>,
    decision: InternalStrategyDecision,
) -> InternalDecisionOutcome {
    let did = decision.decision_id.trim().to_string();
    let sid = decision.strategy_id.trim().to_string();

    // Gate 0: field validation.
    if let Err(blockers) = validate_fields(&decision) {
        return outcome(false, "rejected", &did, &sid, None, blockers);
    }

    // The instrument context is resolved once, here, so the execution domain
    // that owns this order is known before any domain-keyed gate runs. Its
    // error (if any) is surfaced at Gate 7 exactly where it always was.
    let resolved_context = resolve_order_instrument_context(state, &decision.symbol);
    let domain = execution_domain_for_resolved_context(&resolved_context);

    // Gate 1: PT-AUTO-02 per-run signal intake bound, keyed by the domain
    // that owns the order (B2.6): a Crypto order must never consume, or be
    // enqueued onto, the Equity domain's run.
    if state.day_signal_limit_exceeded(domain) {
        return outcome(
            false,
            "day_limit_reached",
            &did,
            &sid,
            None,
            vec![format!(
                "internal decision refused: autonomous day signal limit reached \
                 ({} signals accepted this run); \
                 no further decisions will be accepted until the next run start",
                state.day_signal_count(domain)
            )],
        );
    }

    // Gate 1f: MULTI-SYMBOL-DAY-ORDER-CAP-01 — optional per-symbol daily order
    // count cap (cap #4, design doc §6). Disabled (None) unless
    // MQK_PER_SYMBOL_DAY_ORDER_LIMIT is set; in that case Gate 1 above always
    // passes through unaffected — this is an additive, independent counter.
    if state
        .symbol_day_order_limit_exceeded(domain, &decision.symbol)
        .await
    {
        return outcome(
            false,
            "symbol_day_limit_reached",
            &did,
            &sid,
            None,
            vec![format!(
                "internal decision refused: per-symbol daily order count limit reached for {} \
                 ({} orders accepted this run for this symbol); \
                 no further decisions for this symbol will be accepted until the next run start",
                decision.symbol.trim(),
                state.symbol_day_order_count(domain, &decision.symbol).await
            )],
        );
    }

    // Gate 1e: B6 — TV-04B per-strategy capital budget authorization.
    //
    // Applies the same capital budget gate that the external signal path
    // (POST /api/v1/strategy/signal Gate 1e) enforces.  Without this gate,
    // a strategy can be budget-denied for external signals yet still have its
    // internally-generated bar decisions reach the durable outbox.
    //
    // Placed before Gate 2 (DB) because budget denial is a pure filesystem
    // check — cheaper than DB operations, and budget-denied decisions must
    // never consume DB quota or advance the day signal counter.
    //
    // PolicyNotConfigured → no budget enforcement active; pass through.
    // BudgetAuthorized    → explicit strategy budget authorization; pass.
    // BudgetDenied        → strategy not capital-authorized; fail-closed.
    // PolicyInvalid       → policy configured but structurally invalid; fail-closed.
    {
        use crate::capital_policy::{evaluate_strategy_budget_from_env, StrategyBudgetOutcome};
        let budget = evaluate_strategy_budget_from_env(&sid);
        if !budget.is_signal_safe() {
            let (disposition, blocker) = match &budget {
                StrategyBudgetOutcome::BudgetDenied { reason } => (
                    "budget_denied",
                    format!("internal decision refused: {reason}"),
                ),
                StrategyBudgetOutcome::PolicyInvalid { reason } => (
                    "policy_invalid",
                    format!(
                        "internal decision unavailable: capital allocation policy \
                         is configured but invalid: {reason}"
                    ),
                ),
                _ => (
                    "unavailable",
                    "internal decision unavailable: capital policy evaluation failed".to_string(),
                ),
            };
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on budget denial from
            // the internal (B1C loop) path — same high-value signal as Gate 1e.
            if disposition == "budget_denied" {
                let notifier = state.discord_notifier.clone();
                let env = Some(state.deployment_mode().as_api_label().to_string());
                let blocker_copy = blocker.clone();
                let sid_copy = sid.clone();
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            qty_micros: None,
                            stage: "signal.blocked".to_string(),
                            run_id: None,
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!(
                                "gate=gate_1e_budget path=internal_decision \
                                 strategy={sid_copy} reason={blocker_copy}"
                            )),
                            environment: env,
                            summary: format!(
                                "signal.blocked [budget_denied] internal decision \
                                 strategy={sid_copy} | {blocker_copy}"
                            ),
                            ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                        })
                        .await;
                });
            }
            return outcome(false, disposition, &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 1g: MULTI-SYMBOL-CAPITAL-CAPS-01 — optional per-symbol notional cap
    // (cap #3, design doc §6 "Cap #3 — per_symbol_max_notional_usd").
    //
    // Disabled (NoSizingConstraint) unless MQK_PER_SYMBOL_MAX_NOTIONAL_USD is
    // set to a positive number.
    //
    // Honest gap: implied notional is only computable for limit orders
    // (qty x limit_price). B1C (bar_result_to_decisions) always sets
    // order_type="market" / limit_price=None, so this gate is
    // SizingUnverifiable (pass-through) for every B1C-originated decision —
    // dormant in practice until limit-order support exists for the internal
    // decision path. Proven via direct construction of a limit-order
    // InternalStrategyDecision in scenario_multi_symbol_capital_caps_01.rs.
    //
    // NoSizingConstraint        -> cap disabled or order within cap; pass.
    // SizingUnverifiable        -> market order; pass (honest, cannot deny
    //                               the unmeasured).
    // SizingDeniedPerSymbolCap  -> over cap; "rejected" with a blocker naming
    //                               the cap (design doc §6 cap #3 test).
    {
        use crate::capital_policy::{
            evaluate_per_symbol_notional_cap_from_env, PositionSizingOutcome,
        };
        let sizing = evaluate_per_symbol_notional_cap_from_env(
            &decision.symbol,
            decision.qty,
            decision.limit_price,
        );
        if let PositionSizingOutcome::SizingDeniedPerSymbolCap {
            symbol,
            implied_notional_usd,
            cap_usd,
        } = &sizing
        {
            let blocker = format!(
                "internal decision refused: symbol '{symbol}' implied notional \
                 ${implied_notional_usd:.2} exceeds per_symbol_max_notional_usd=${cap_usd:.2} \
                 (MQK_PER_SYMBOL_MAX_NOTIONAL_USD)"
            );
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on per-symbol
            // notional cap denial — same high-value signal as Gate 1e/1f.
            let notifier = state.discord_notifier.clone();
            let env = Some(state.deployment_mode().as_api_label().to_string());
            let symbol_owned = symbol.clone();
            let blocker_copy = blocker.clone();
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        qty_micros: None,
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: Some(symbol_owned.clone()),
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!(
                            "gate=gate_1g_per_symbol_notional_cap path=internal_decision \
                             reason={blocker_copy}"
                        )),
                        environment: env,
                        summary: format!(
                            "signal.blocked [sizing_denied_per_symbol_cap] internal decision \
                             symbol={symbol_owned} | {blocker_copy}"
                        ),
                        ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                    })
                    .await;
            });
            return outcome(false, "rejected", &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 1h: ETF-RISK-CLOSURE-01 / ETF-RISK-EXTERNAL-SIGNAL-GATE-01 —
    // optional per-sector live gross exposure cap
    // (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`).
    //
    // Default-off: an unset/empty env var disables this gate entirely and
    // the decision path never touches the DB or the instrument registry for
    // this check (mirrors Gate 1g's MQK_PER_SYMBOL_MAX_NOTIONAL_USD shape —
    // same env-var-driven, no-cap-means-no-check pattern).
    //
    // The registry/snapshot/marks glue and the fail-closed rules (missing
    // snapshot/DB/mark/NAV never fabricate a price or treat a gap as zero)
    // live in `capital_policy::sector_risk_gate::evaluate_sector_risk_gate`,
    // shared with the external signal path's gate in `routes/strategy.rs` —
    // one mechanism, two callers, so behavior cannot drift between an
    // internally-generated order and an externally-submitted signal.
    {
        let result = crate::capital_policy::sector_risk_gate::evaluate_sector_risk_gate(
            state,
            decision.symbol.trim(),
            decision.side.trim(),
            decision.qty,
        )
        .await;

        if !result.allowed {
            let prefix = if matches!(
                result.reason_code.as_str(),
                "sector_config_invalid" | "unavailable"
            ) {
                "internal decision unavailable"
            } else {
                "internal decision refused"
            };
            let blocker = format!(
                "{prefix}: {}",
                result
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("sector risk gate denied ({})", result.reason_code))
            );
            let notifier = state.discord_notifier.clone();
            let env = Some(state.deployment_mode().as_api_label().to_string());
            let symbol_owned = decision.symbol.trim().to_string();
            let blocker_copy = blocker.clone();
            let reason_code_copy = result.reason_code.clone();
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        qty_micros: None,
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: Some(symbol_owned.clone()),
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!(
                            "gate=gate_1h_sector_risk path=internal_decision \
                             reason={blocker_copy}"
                        )),
                        environment: env,
                        summary: format!(
                            "signal.blocked [{reason_code_copy}] internal decision \
                             symbol={symbol_owned} | {blocker_copy}"
                        ),
                        ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                    })
                    .await;
            });
            return outcome(false, &result.reason_code, &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 2: DB must be present.
    let Some(db) = state.db.as_ref() else {
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            None,
            vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        );
    };

    // Gate 3: strategy must be registered and enabled in sys_strategy_registry.
    match mqk_db::fetch_strategy_registry_entry(db, &sid).await {
        Ok(Some(record)) if record.enabled => {
            // Pass — registered and enabled.
        }
        Ok(Some(_record)) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is registered but disabled \
                     in the strategy registry"
                )],
            );
        }
        Ok(None) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is not registered \
                     in the strategy registry"
                )],
            );
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: registry lookup failed: {err}"
                )],
            );
        }
    }

    // Gate 3b: STRATEGY-PROMOTION-REGISTRY-01D — strategy must be
    // paper-promoted (active_paper) for this exact
    // (strategy_id, symbol, timeframe_secs) identity.
    //
    // Registered + enabled (Gate 3 above) is necessary but never
    // sufficient for paper trading: a strategy can be registered and
    // enabled yet have no promotion record at all, or a shadow/demoted/
    // retired/rejected/expired one, and must still be refused here.
    // Shared with the external signal path (routes/strategy.rs) via
    // `promotion_gate::evaluate_paper_promotion_gate` — one mechanism, two
    // callers, so promotion enforcement cannot drift between an
    // internally-generated order and an externally-submitted signal.
    {
        let promotion = crate::promotion_gate::evaluate_paper_promotion_gate(
            db,
            crate::promotion_gate::PromotionRunMode::from(state.deployment_mode()),
            &sid,
            decision.symbol.trim(),
            decision.timeframe_secs,
            crate::promotion_gate::SemanticProvenance::Fingerprint(Some(
                decision.strategy_semantic_fingerprint.as_str(),
            )),
        )
        .await;
        if !promotion.paper_tradable {
            let disposition = match promotion.reason_code {
                mqk_db::PromotionReasonCode::PromotionDbUnavailable
                | mqk_db::PromotionReasonCode::PromotionQueryFailed => "unavailable",
                other => other.code(),
            };
            return outcome(
                false,
                disposition,
                &did,
                &sid,
                None,
                vec![format!("internal decision refused: {}", promotion.blocker)],
            );
        }
    }

    // Gate 4: strategy must not be actively suppressed.
    //
    // Uses a targeted per-strategy query so the decision seam does not load
    // all suppressions for all strategies on every call.  Fail-closed:
    // if the suppression truth is unavailable the decision is refused.
    match mqk_db::fetch_active_suppression_for_strategy(db, &sid).await {
        Ok(Some(sup)) => {
            return outcome(
                false,
                "suppressed",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is suppressed \
                     ({}: {})",
                    sup.trigger_domain, sup.trigger_reason
                )],
            );
        }
        Ok(None) => {
            // No active suppression — pass.
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: suppression check failed: {err}"
                )],
            );
        }
    }

    // Gate 5: durable arm state must be ARMED.
    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((s, r))) => (s, r),
        Ok(None) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![
                    "internal decision refused: durable arm state is not armed; \
                      fresh systems default to disarmed until explicitly armed"
                        .to_string(),
                ],
            );
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: arm-state truth could not be loaded: {err}"
                )],
            );
        }
    };

    if durable_arm_state != "ARMED" {
        let blocker = match durable_arm_reason.as_deref() {
            Some("OperatorHalt") => {
                "internal decision refused: durable arm state is halted".to_string()
            }
            Some(reason) => {
                format!("internal decision refused: durable arm state is disarmed ({reason})")
            }
            None => "internal decision refused: durable arm state is not armed".to_string(),
        };
        return outcome(false, "rejected", &did, &sid, None, vec![blocker]);
    }

    // Gate 6: the owning domain's active run must exist and be "running".
    let status = match state.current_status_snapshot(domain).await {
        Ok(s) => s,
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![err.to_string()],
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            None,
            vec!["internal decision refused: no active durable run is available".to_string()],
        );
    };

    if status.state != "running" {
        let mut blockers = vec![format!(
            "internal decision refused: runtime state '{}' is not accepting decisions",
            status.state
        )];
        if let Some(note) = status.notes {
            blockers.push(note);
        }
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            Some(active_run_id),
            blockers,
        );
    }

    // Gate 7: resolve the exact trading-instrument context before the
    // durable enqueue. Crypto economics are frozen into order_json here so
    // dispatch/restart never depend on later ambient registry state.
    let instrument_context = match resolved_context {
        Ok(context) => context,
        Err(OrderInstrumentContextError::Unavailable(blocker)) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                Some(active_run_id),
                vec![blocker],
            );
        }
        Err(OrderInstrumentContextError::Rejected(blocker)) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                Some(active_run_id),
                vec![blocker],
            );
        }
    };

    // Gate 7b (D3, V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): an unresolved
    // options-lifecycle event (OPEXC/OPASN/OPEXP durably ingested by D1, not
    // yet applied by D2) for this exact symbol must block real economic
    // action on it. Scoped by the configured Alpaca account's own identity
    // (mirrors B6/D1's `broker_account_id` pattern) so this account's
    // pending evidence never leaks into a check for a different account.
    // Durable unresolved state fences regardless of whether the lifecycle
    // fetcher exists in this process: without one the account is unknown and
    // any unreconciled event of this deployment mode fences the symbol.
    match crate::state::option_lifecycle_pending_gate::check_symbol_fence(
        db,
        state.option_lifecycle_activity_fetcher.as_ref(),
        state.deployment_mode().as_api_label(),
        &decision.symbol,
    )
    .await
    {
        Ok(status) if status.must_fail_closed() => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                Some(active_run_id),
                vec![format!(
                    "internal decision refused: symbol '{}': {}",
                    decision.symbol,
                    status.explanation()
                )],
            );
        }
        Ok(_) => {}
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                Some(active_run_id),
                vec![format!("internal decision unavailable: {err}")],
            );
        }
    }

    if let Err(blocker) = non_equity_explicit_size_gate(
        &instrument_context.asset_class,
        decision.qty,
        std::env::var("MQK_STRATEGY_TARGET_QTY").ok().as_deref(),
        std::env::var("MQK_STRATEGY_MAX_TARGET_QTY").ok().as_deref(),
        std::env::var("MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD")
            .ok()
            .as_deref(),
    ) {
        return outcome(
            false,
            "rejected",
            &did,
            &sid,
            Some(active_run_id),
            vec![blocker],
        );
    }

    let mut decision = decision;
    match admit_time_in_force(&instrument_context.asset_class, &decision.time_in_force) {
        Ok(tif) => decision.time_in_force = tif,
        Err(blocker) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                Some(active_run_id),
                vec![blocker],
            );
        }
    }

    let order_json = build_order_json(&decision, &instrument_context);

    match mqk_db::outbox_enqueue_new_order_for_running_run(db, active_run_id, &did, order_json)
        .await
    {
        Ok(mqk_db::OutboxEnqueueOutcome::Enqueued) => {
            // PT-AUTO-02: count only new enqueues; duplicates do not consume quota.
            state.increment_day_signal_count(domain);
            // MULTI-SYMBOL-DAY-ORDER-CAP-01: per-symbol counterpart (cap #4),
            // incremented alongside the account-wide counter above.
            state
                .increment_symbol_day_order_count(domain, &decision.symbol)
                .await;
            outcome(true, "accepted", &did, &sid, Some(active_run_id), vec![])
        }
        Ok(mqk_db::OutboxEnqueueOutcome::Duplicate) => outcome(
            false,
            "duplicate",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!(
                "decision_id '{did}' already exists in outbox; no new row was created"
            )],
        ),
        Ok(mqk_db::OutboxEnqueueOutcome::RunModeForbidsNewOrder { run_mode }) => outcome(
            false,
            "order_authority_denied",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!(
                "internal decision refused: durable run mode '{run_mode}' may not create new economic orders"
            )],
        ),
        Ok(mqk_db::OutboxEnqueueOutcome::RunNotRunning { actual_status }) => outcome(
            false,
            "unavailable",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!(
                "internal decision refused: durable run status is '{actual_status}', not RUNNING"
            )],
        ),
        Err(err) => outcome(
            false,
            "unavailable",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!("outbox enqueue failed: {err}")],
        ),
    }
}

#[cfg(test)]
mod m6_trading_registry_snapshot_writer_tests {
    use super::*;

    /// Whole-unit test quantity (`1` == one share == `QTY_MICROS_SCALE` raw).
    fn q(units: i64) -> mqk_schemas::QtyMicros {
        mqk_schemas::QtyMicros::from_whole_units(units).unwrap()
    }
    use std::collections::BTreeMap;

    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
        InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
    };

    fn btc_registry() -> InstrumentRegistryV2 {
        InstrumentRegistryV2 {
            schema_version: 1,
            instruments: vec![InstrumentDefinitionV2 {
                instrument_id: "crypto:GLOBAL:BTCUSD".to_string(),
                symbol: "BTC/USD".to_string(),
                asset_class: "crypto".to_string(),
                instrument_kind: None,
                venue: Some("GLOBAL".to_string()),
                currency: "USD".to_string(),
                quote_currency: Some("USD".to_string()),
                provider_symbols: BTreeMap::from([("kraken".to_string(), "XBTUSD".to_string())]),
                broker_symbols: BTreeMap::from([("alpaca".to_string(), "BTC/USD".to_string())]),
                enabled: false,
                paper_trading_enabled: true,
                live_trading_enabled: false,
                timeframes: vec!["5m".to_string()],
                contract: Some(ContractDefinitionV2::CryptoPair {
                    base: "BTC".to_string(),
                    quote: "USD".to_string(),
                }),
                metadata: InstrumentMetadataV2::default(),
                notes: Some("M6 trading-registry writer proof".to_string()),
                allow_enabled_non_equity_for_testing: false,
                economics: Some(InstrumentEconomicsMetadataV2 {
                    contract_multiplier: None,
                    initial_margin_micros: None,
                    maintenance_margin_micros: None,
                    quantity_increment_micros: Some(100),
                    min_trade_qty_micros: Some(100),
                    price_tick_micros: Some(1_000_000),
                    session_profile: Some(SESSION_PROFILE_CRYPTO_24_7.to_string()),
                }),
            }],
        }
    }

    fn decision() -> InternalStrategyDecision {
        InternalStrategyDecision {
            decision_id: "m6-registry-writer-test".to_string(),
            strategy_id: "test-strategy".to_string(),
            symbol: "BTC/USD".to_string(),
            timeframe_secs: 300,
            strategy_semantic_fingerprint: "test-fingerprint".to_string(),
            side: "buy".to_string(),
            qty: q(1),
            order_type: "market".to_string(),
            time_in_force: "gtc".to_string(),
            limit_price: None,
        }
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_stamps_crypto_economics() {
        let registry = btc_registry();

        let context = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .expect("valid BTC/USD Paper registry row must resolve");

        assert_eq!(context.asset_class, "crypto");

        let order = build_order_json(&decision(), &context);

        assert_eq!(
            order.get("asset_class").and_then(|v| v.as_str()),
            Some("crypto")
        );

        let economics = order
            .get("instrument_economics")
            .expect("Crypto order must carry durable economics");

        assert_eq!(
            economics.get("source").and_then(|v| v.as_str()),
            Some("registry_v2")
        );

        assert_eq!(
            economics.get("authority").and_then(|v| v.as_str()),
            Some("MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH")
        );

        assert_eq!(
            economics
                .get("min_trade_qty_micros")
                .and_then(|v| v.as_i64()),
            Some(100)
        );

        assert_eq!(
            economics.get("tick_size_micros").and_then(|v| v.as_i64()),
            Some(1_000_000)
        );

        assert_eq!(
            economics
                .get("quantity_increment_micros")
                .and_then(|v| v.as_i64()),
            Some(100)
        );
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_absent_symbol_preserves_equity_shape() {
        let registry = btc_registry();

        let context = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "AAPL",
            true,
        )
        .expect("symbol absent from additive v2 source keeps legacy Equity");

        assert_eq!(context.asset_class, "equity");
        assert!(context.economics_snapshot.is_none());
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_non_paper_mode_fails_closed() {
        let registry = btc_registry();

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::LiveShadow,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Rejected(message)
                if message.contains("only in Paper mode")
        ));
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_missing_broker_identity_fails_closed() {
        let mut registry = btc_registry();
        registry.instruments[0].broker_symbols.clear();

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Rejected(message)
                if message.contains("no Alpaca broker symbol")
        ));
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_live_enabled_row_fails_closed() {
        let mut registry = btc_registry();
        registry.instruments[0].live_trading_enabled = true;

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Rejected(message)
                if message.contains("live_trading_enabled=true")
        ));
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_non_alpaca_broker_fails_closed() {
        let registry = btc_registry();

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Paper),
            "BTC/USD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Rejected(message)
                if message.contains("requires the configured broker")
        ));
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_unknown_symbol_does_not_become_equity() {
        let registry = btc_registry();

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTCUSD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Rejected(message)
                if message.contains("neither present")
        ));
    }

    #[test]
    fn m6_trading_registry_snapshot_writer_test_only_bypass_fails_closed() {
        let mut registry = btc_registry();

        registry.instruments[0].allow_enabled_non_equity_for_testing = true;

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            OrderInstrumentContextError::Unavailable(message)
                if message.contains("test-only validator bypasses")
        ));
    }

    #[test]
    fn non_account_currency_crypto_instrument_is_refused_not_valued_as_usd() {
        // Same row shape as the admitted BTC/USD fixture, only the currency
        // differs: the refusal must come from the currency check, not from
        // any other registry rule.
        let mut registry = btc_registry();
        {
            let row = &mut registry.instruments[0];
            row.instrument_id = "crypto:GLOBAL:BTCEUR".to_string();
            row.symbol = "BTC/EUR".to_string();
            row.currency = "EUR".to_string();
            row.quote_currency = Some("EUR".to_string());
            row.broker_symbols = BTreeMap::from([("alpaca".to_string(), "BTC/EUR".to_string())]);
            row.contract = Some(ContractDefinitionV2::CryptoPair {
                base: "BTC".to_string(),
                quote: "EUR".to_string(),
            });
        }

        let err = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/EUR",
            false,
        )
        .unwrap_err();

        assert!(
            matches!(
                &err,
                OrderInstrumentContextError::Rejected(message)
                    if message.contains("currency_conversion_unsupported")
                        && message.contains("'EUR'")
            ),
            "{err:?}"
        );
    }

    // -----------------------------------------------------------------
    // CUTOVER-1D-A3-4: writer -> order_json -> runtime decoder agreement
    // -----------------------------------------------------------------

    #[test]
    fn a3_4_order_json_qty_writer_and_runtime_reader_agree_on_fractional_qty() {
        let registry = btc_registry();
        let context = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .expect("valid BTC/USD Paper registry row must resolve");

        let cases: [(i64, serde_json::Value); 4] = [
            (100, serde_json::json!("0.0001")),
            (123_400, serde_json::json!("0.1234")),
            (1_500_000, serde_json::json!("1.5")),
            (2_000_000, serde_json::json!(2)), // whole => historical integer shape
        ];
        for (raw, expected_json) in cases {
            let mut d = decision();
            d.qty = mqk_schemas::QtyMicros::new(raw);
            let order = build_order_json(&d, &context);
            assert_eq!(order["qty"], expected_json, "writer shape for raw={raw}");
            assert!(!order["qty"].is_f64(), "never a floating-point JSON number");
            let req = mqk_runtime::orchestrator::build_validated_submit_request("oid", &order)
                .expect("runtime decoder must accept the writer's payload");
            assert_eq!(
                req.quantity, d.qty,
                "runtime reads exactly what was written"
            );
        }

        // An exact but off-increment fractional quantity is written exactly
        // and then REFUSED by the runtime's registry-economics gate (never
        // rounded to an increment).
        let mut off = decision();
        off.qty = mqk_schemas::QtyMicros::new(123_456);
        let order = build_order_json(&off, &context);
        assert_eq!(order["qty"], serde_json::json!("0.123456"));
        assert!(mqk_runtime::orchestrator::build_validated_submit_request("oid", &order).is_err());
    }

    #[test]
    fn a3_4_one_qty_micro_mutation_changes_the_durable_order_qty() {
        let registry = btc_registry();
        let context = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .expect("valid BTC/USD Paper registry row must resolve");
        let write = |raw: i64| {
            let mut d = decision();
            d.qty = mqk_schemas::QtyMicros::new(raw);
            build_order_json(&d, &context)
        };
        let base = write(100);
        for neighbour in [99, 101] {
            let other = write(neighbour);
            assert_ne!(base["qty"], other["qty"], "0.0001 vs raw {neighbour}");
            assert_ne!(base, other);
        }
        assert_eq!(base["qty"], serde_json::json!("0.0001"));
        assert_eq!(write(101)["qty"], serde_json::json!("0.000101"));
        assert_eq!(write(99)["qty"], serde_json::json!("0.000099"));
        assert_eq!(
            base,
            write(100),
            "identical fractional replay is byte-stable"
        );
    }

    // -----------------------------------------------------------------
    // RC-M6-B: crypto time-in-force is decided at admission
    // -----------------------------------------------------------------

    fn btc_context() -> DurableOrderInstrumentContext {
        resolve_order_instrument_context_from_registry(
            &btc_registry(),
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .expect("valid BTC/USD Paper registry row must resolve")
    }

    #[test]
    fn rcm6b_crypto_explicit_gtc_and_ioc_survive_the_runtime_decoder_and_adapter() {
        let context = btc_context();
        for order_type in ["market", "limit"] {
            for tif in ["gtc", "ioc"] {
                let mut d = decision();
                d.qty = mqk_schemas::QtyMicros::new(100);
                d.order_type = order_type.to_string();
                d.limit_price = (order_type == "limit").then_some(60_000_000_000);
                d.time_in_force = tif.to_string();
                d.time_in_force = admit_time_in_force(&context.asset_class, &d.time_in_force)
                    .unwrap_or_else(|e| {
                        panic!("crypto {order_type}/{tif} must be admissible: {e}")
                    });
                assert_eq!(d.time_in_force, tif, "explicit TIF is never rewritten");

                let order = build_order_json(&d, &context);
                assert_eq!(order["time_in_force"], tif);
                let req = mqk_runtime::orchestrator::build_validated_submit_request("oid", &order)
                    .expect("runtime decoder must accept the admitted payload");
                assert_eq!(req.time_in_force, tif);
                mqk_broker_alpaca::validate_alpaca_crypto_order_tif(&req.time_in_force)
                    .expect("the admitted TIF must satisfy the Alpaca crypto adapter");
            }
        }
    }

    #[test]
    fn rcm6b_crypto_day_from_the_generic_translator_is_refused_not_rewritten() {
        // The bar->decision translator emits market + "day" for every asset; a
        // crypto order therefore fails closed until a crypto policy surface
        // explicitly emits gtc/ioc.
        let context = btc_context();
        let err = admit_time_in_force(&context.asset_class, "day")
            .expect_err("crypto day must be refused");
        assert!(err.contains("'day'"), "{err}");
    }

    #[test]
    fn rcm6b_crypto_time_in_force_table() {
        let adm = |tif: &str| admit_time_in_force("crypto", tif);
        assert_eq!(adm("gtc").as_deref(), Ok("gtc"));
        assert_eq!(adm("ioc").as_deref(), Ok("ioc"));
        // Trim / case normalization follows the existing contract.
        assert_eq!(adm(" GTC ").as_deref(), Ok("gtc"));
        assert_eq!(adm("IOC").as_deref(), Ok("ioc"));
        // No day -> gtc / ioc rewrite in any spelling.
        for tif in [
            "day", " DAY ", "Day", "fok", "opg", "cls", "", "  ", "gtd", "gtc1",
        ] {
            assert!(adm(tif).is_err(), "{tif:?} must be refused");
        }
    }

    #[test]
    fn rcm6b_equity_time_in_force_is_untouched() {
        for tif in ["day", "gtc", "ioc", "fok"] {
            assert_eq!(admit_time_in_force("equity", tif).as_deref(), Ok(tif));
        }
        assert_eq!(admit_time_in_force("equity", " DAY ").as_deref(), Ok("day"));
    }

    // -----------------------------------------------------------------
    // B5: Crypto TIF authority lives upstream (construction seam); decision.rs
    // only validates.
    // -----------------------------------------------------------------

    use crate::state::crypto_execution_policy::{
        crypto_execution_policy_fingerprint, strategy_time_in_force, CryptoTimeInForce,
        CryptoTimeInForceConfig,
    };

    fn b5_result(symbol: &str, target_micros: i64) -> mqk_strategy::StrategyBarResult {
        mqk_strategy::StrategyBarResult {
            spec: mqk_strategy::StrategySpec::new("intraday_scalper", 300),
            semantic_fingerprint: "fp".to_string(),
            intents: mqk_strategy::StrategyIntents {
                mode: mqk_strategy::IntentMode::Live,
                output: mqk_strategy::StrategyOutput {
                    targets: vec![mqk_strategy::TargetPosition::new(
                        symbol,
                        mqk_schemas::QtyMicros::new(target_micros),
                    )],
                },
            },
        }
    }

    /// The exact resolver shape production wires: asset class -> policy TIF.
    fn b5_resolver(
        class: &'static str,
        config: CryptoTimeInForceConfig,
    ) -> impl Fn(&str) -> Result<String, String> {
        move |_symbol| strategy_time_in_force(class, config).map(str::to_string)
    }

    fn b5_construct(
        class: &'static str,
        config: CryptoTimeInForceConfig,
    ) -> Vec<InternalStrategyDecision> {
        bar_result_to_decisions_with_tif(
            &b5_result("BTC/USD", 100_000),
            Uuid::nil(),
            1_000,
            &BTreeMap::new(),
            b5_resolver(class, config),
        )
    }

    #[test]
    fn b5_construction_seam_emits_configured_gtc_and_ioc_into_the_decision() {
        for (tif, label) in [
            (CryptoTimeInForce::Gtc, "gtc"),
            (CryptoTimeInForce::Ioc, "ioc"),
        ] {
            let ds = b5_construct("crypto", CryptoTimeInForceConfig::Explicit(tif));
            assert_eq!(ds.len(), 1);
            assert_eq!(ds[0].time_in_force, label);
            // The emitted value passes decision.rs validate-only admission
            // unchanged and reaches the durable order JSON.
            assert_eq!(
                admit_time_in_force("crypto", &ds[0].time_in_force).as_deref(),
                Ok(label)
            );
            let order = build_order_json(&ds[0], &btc_context());
            assert_eq!(order["time_in_force"], label);
            // Provenance persists the canonical value of what was constructed.
            assert_eq!(
                order["crypto_execution_policy_fingerprint"],
                serde_json::Value::String(crypto_execution_policy_fingerprint(
                    CryptoTimeInForceConfig::Explicit(tif)
                ))
            );
        }
    }

    #[test]
    fn b5_unconfigured_or_invalid_policy_constructs_no_crypto_decision() {
        for config in [
            CryptoTimeInForceConfig::Unconfigured,
            CryptoTimeInForceConfig::Invalid,
        ] {
            assert!(b5_construct("crypto", config).is_empty());
        }
    }

    #[test]
    fn b5_decision_layer_refuses_day_and_never_rewrites_it() {
        // A crypto decision that reaches admission carrying `day` (or any
        // non-gtc/ioc) is refused, not rewritten -- there is no policy in
        // this layer to rewrite it with.
        for tif in ["day", "fok", "opg", "cls", "", "unknown"] {
            let err = admit_time_in_force("crypto", tif).expect_err("must refuse");
            assert!(err.contains("internal decision refused"), "{err}");
        }
    }

    #[test]
    fn b5_gtc_ioc_change_decision_identity_case_whitespace_does_not() {
        let id = |raw: &str| {
            let config =
                crate::state::crypto_execution_policy::crypto_time_in_force_config_from_env_value(
                    Some(raw),
                );
            b5_construct("crypto", config)[0].decision_id.clone()
        };
        assert_ne!(
            id("gtc"),
            id("ioc"),
            "gtc<->ioc is a different economic intent"
        );
        assert_eq!(id("gtc"), id(" GTC "));
        assert_eq!(id("ioc"), id("Ioc\n"));
    }

    #[test]
    fn b5_equity_construction_and_identity_are_byte_for_byte_unchanged() {
        let generic = bar_result_to_decisions(
            &b5_result("AAPL", 10_000_000),
            Uuid::nil(),
            1_000,
            &BTreeMap::new(),
        );
        for config in [
            CryptoTimeInForceConfig::Unconfigured,
            CryptoTimeInForceConfig::Invalid,
            CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Gtc),
            CryptoTimeInForceConfig::Explicit(CryptoTimeInForce::Ioc),
        ] {
            let with_policy = bar_result_to_decisions_with_tif(
                &b5_result("AAPL", 10_000_000),
                Uuid::nil(),
                1_000,
                &BTreeMap::new(),
                b5_resolver("equity", config),
            );
            assert_eq!(with_policy.len(), 1);
            assert_eq!(with_policy[0].time_in_force, "day");
            assert_eq!(with_policy[0].decision_id, generic[0].decision_id);
        }
        for tif in ["day", "gtc", "ioc", "fok"] {
            assert_eq!(admit_time_in_force("equity", tif).as_deref(), Ok(tif));
        }
    }

    #[test]
    fn b5_decision_rs_holds_no_crypto_policy_or_environment_authority() {
        // Production (pre-test) section of this file only.
        let production = include_str!("decision.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "MQK_CRYPTO_TIME_IN_FORCE",
            "CRYPTO_TIME_IN_FORCE_ENV",
            "configured_crypto_time_in_force",
            "crypto_time_in_force_config_from_env",
            "resolve_admitted_time_in_force",
            "CryptoTimeInForceConfig",
        ] {
            assert!(
                !production.contains(forbidden),
                "decision.rs production code must not reference `{forbidden}`: Crypto TIF \
                 authority is upstream of decision.rs"
            );
        }
    }

    #[test]
    fn a3_4_equity_order_json_qty_stays_a_whole_integer() {
        let ctx = DurableOrderInstrumentContext::legacy_equity();
        let mut d = decision();
        d.symbol = "AAPL".to_string();
        d.qty = q(10);
        let order = build_order_json(&d, &ctx);
        assert_eq!(order["qty"], serde_json::json!(10));
        assert_eq!(
            order["qty"].to_string(),
            "10",
            "byte-identical to the i64 form"
        );
    }

    #[test]
    fn a3_4_position_book_retains_fractional_positions() {
        let positions = vec![
            mqk_runtime::observability::PositionSnapshot {
                symbol: "BTC/USD".to_string(),
                net_qty: mqk_schemas::QtyMicros::new(500_000),
            },
            mqk_runtime::observability::PositionSnapshot {
                symbol: "AAPL".to_string(),
                net_qty: q(7),
            },
        ];
        let book = position_book_from_snapshot(&positions);
        assert_eq!(book.len(), 2, "no position may be dropped");
        assert_eq!(book["BTC/USD"], mqk_schemas::QtyMicros::new(500_000));
        assert_eq!(book["AAPL"], q(7));
    }

    #[test]
    fn a3_4_fractional_target_and_current_delta_is_exact_and_checked() {
        let result = mqk_strategy::StrategyBarResult {
            spec: mqk_strategy::StrategySpec::new("intraday_scalper", 300),
            semantic_fingerprint: "fp".to_string(),
            intents: mqk_strategy::StrategyIntents {
                mode: mqk_strategy::IntentMode::Live,
                output: mqk_strategy::StrategyOutput {
                    targets: vec![mqk_strategy::TargetPosition::new(
                        "BTC/USD",
                        mqk_schemas::QtyMicros::new(350),
                    )],
                },
            },
        };
        let mut book = BTreeMap::new();
        book.insert("BTC/USD".to_string(), mqk_schemas::QtyMicros::new(250));
        let ds = bar_result_to_decisions(&result, Uuid::nil(), 1_000, &book);
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].side, "buy");
        assert_eq!(ds[0].qty, mqk_schemas::QtyMicros::new(100));

        // Overflowing target - current refuses (no decision), never wraps.
        book.insert("BTC/USD".to_string(), mqk_schemas::QtyMicros::new(-1));
        let mut big = result.clone();
        big.intents.output.targets[0].qty = mqk_schemas::QtyMicros::new(i64::MAX);
        assert!(bar_result_to_decisions(&big, Uuid::nil(), 1_000, &book).is_empty());
    }

    #[test]
    fn a3_4_crypto_sizing_is_validated_against_registry_economics_without_choosing_it() {
        use mqk_strategy::{SizingError, TargetSizing};
        let registry = btc_registry();
        let context = resolve_order_instrument_context_from_registry(
            &registry,
            crate::state::DeploymentMode::Paper,
            Some(crate::state::BrokerKind::Alpaca),
            "BTC/USD",
            false,
        )
        .unwrap();
        let econ = context.economics_snapshot.as_ref().expect("economics");
        let inc = econ["quantity_increment_micros"].as_i64().unwrap();
        let min = econ["min_trade_qty_micros"].as_i64().unwrap();

        let ok = TargetSizing::resolve(
            mqk_execution::AssetClass::Crypto,
            Some("0.0001"),
            None,
            None,
        )
        .unwrap();
        assert!(ok.validate_against(inc, min).is_ok());
        let bad = TargetSizing::resolve(
            mqk_execution::AssetClass::Crypto,
            Some("0.00015"),
            None,
            None,
        )
        .unwrap();
        assert!(
            bad.validate_against(inc, min).is_err(),
            "off-increment refused, not rounded"
        );
        assert_eq!(
            TargetSizing::resolve(mqk_execution::AssetClass::Crypto, None, None, None),
            Err(SizingError::MissingExplicitSize {
                asset_class: mqk_execution::AssetClass::Crypto
            }),
            "no default 1 BTC"
        );
    }

    #[test]
    fn a3_5_non_equity_size_gate_fails_closed_and_never_defaults_to_one_unit() {
        let gate = |class: &str, qty: i64, target: Option<&str>| {
            non_equity_explicit_size_gate(
                class,
                mqk_schemas::QtyMicros::new(qty),
                target,
                None,
                None,
            )
        };
        // Equity is untouched.
        assert!(gate("equity", 1_000_000, None).is_ok());
        // Crypto with no explicit size: refused (no default 1 BTC).
        assert!(gate("crypto", 1_000_000, None).is_err());
        assert!(gate("crypto", 100, Some("  ")).is_err());
        assert!(gate("crypto", 100, Some("0.0000001")).is_err());
        assert!(gate("crypto", 100, Some("abc")).is_err());
        // Explicit exact size admits at most that size.
        assert!(gate("crypto", 100, Some("0.0001")).is_ok());
        assert!(gate("crypto", 50, Some("0.0001")).is_ok());
        assert!(
            gate("crypto", 1_000_000, Some("0.0001")).is_err(),
            "a whole-share fallback quantity must be refused"
        );
        // Unsupported asset classes refuse.
        assert!(gate("option", 100, Some("1")).is_err());
        // A max cap below the target lowers the admitted ceiling.
        assert!(non_equity_explicit_size_gate(
            "crypto",
            mqk_schemas::QtyMicros::new(200),
            Some("0.0005"),
            Some("0.0001"),
            None,
        )
        .is_err());
    }
}

#[cfg(test)]
mod a3_signal_evidence_tests {
    use super::*;

    fn total_of(qtys: &[i64]) -> Option<QtyMicros> {
        let targets: Vec<mqk_strategy::TargetPosition> = qtys
            .iter()
            .map(|q| mqk_strategy::TargetPosition::new("BTC/USD", QtyMicros::new(*q)))
            .collect();
        sum_target_qty(targets.iter())
    }

    /// A: a 0.0001 strategy signal is raw 100 in the exact in-memory state and
    /// journals as exact, generated evidence -- never absent.
    #[test]
    fn a3_fractional_strategy_signal_is_exact_raw_100_end_to_end() {
        let total = total_of(&[100]);
        assert_eq!(total, Some(QtyMicros::new(100)));
        assert!(signal_generated(total));
        let last = LastBarSignal::from_total(total);
        assert_eq!(last, LastBarSignal::Evaluated(QtyMicros::new(100)));
        assert_eq!(last.exact().map(QtyMicros::raw), Some(100));
        assert_eq!(
            mqk_db::SignalQtyEvidence::evaluated(total),
            mqk_db::SignalQtyEvidence::Exact(QtyMicros::new(100))
        );
    }

    /// B/C: no-dispatch, flat, fractional, and overflowed are four distinct
    /// states; only no-dispatch is `null` on V1.
    #[test]
    fn a3_no_dispatch_flat_fractional_overflow_are_distinct() {
        let states = [
            LastBarSignal::NoDispatch,
            LastBarSignal::from_total(total_of(&[])),
            LastBarSignal::from_total(total_of(&[100])),
            LastBarSignal::from_total(total_of(&[i64::MAX, 1])),
            LastBarSignal::from_total(Some(QtyMicros::new(i64::MIN))),
        ];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(states[1], LastBarSignal::Evaluated(QtyMicros::ZERO));
        assert!(states[1].is_flat());
        assert_eq!(states[3], LastBarSignal::TotalOverflowed);
        assert_eq!(states[0].v1_whole_units(), Ok(None));
        assert_eq!(states[1].v1_whole_units(), Ok(Some(0)));
        assert_eq!(states[2].v1_whole_units(), Err(NotRepresentableInV1));
        assert_eq!(states[3].v1_whole_units(), Err(NotRepresentableInV1));
        assert_eq!(
            LastBarSignal::from_total(total_of(&[5_000_000])).v1_whole_units(),
            Ok(Some(5))
        );
    }

    /// An overflowed target sum is journaled as evaluated-without-quantity and
    /// generated=true, never as a provably-flat or pre-dispatch-absent row.
    #[test]
    fn a3_overflowed_total_is_generated_and_not_absent() {
        let total = total_of(&[i64::MAX, 1]);
        assert_eq!(total, None);
        assert!(signal_generated(total));
        assert_eq!(
            mqk_db::SignalQtyEvidence::evaluated(total),
            mqk_db::SignalQtyEvidence::TotalOverflowed
        );
        assert_ne!(
            mqk_db::SignalQtyEvidence::evaluated(total),
            mqk_db::SignalQtyEvidence::NotEvaluated
        );
    }
}
