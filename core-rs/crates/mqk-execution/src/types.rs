use serde::{Deserialize, Serialize};

/// Strategy output is a set of target positions.
///
/// The engine decides on a target portfolio state; execution is computed as
/// deltas against broker positions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOutput {
    pub targets: Vec<TargetPosition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetPosition {
    pub symbol: String,
    /// Signed quantity. +long, -short.
    pub qty: i64,
}

impl StrategyOutput {
    #[inline]
    pub fn new(targets: Vec<TargetPosition>) -> Self {
        Self { targets }
    }
}

impl TargetPosition {
    #[inline]
    pub fn new<S: Into<String>>(symbol: S, qty: i64) -> Self {
        Self {
            symbol: symbol.into(),
            qty,
        }
    }
}

impl ExecutionDecision {
    /// Convenience accessor for tests/callers that want the order list.
    /// Returns an empty slice for Noop/HaltAndDisarm.
    #[inline]
    pub fn intents(&self) -> &[ExecutionIntent] {
        match self {
            ExecutionDecision::PlaceOrders(intents) => intents.as_slice(),
            ExecutionDecision::Noop | ExecutionDecision::HaltAndDisarm { .. } => &[],
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// An order intent is the minimal representation of “place an order”
/// that strategy code can generate without knowing broker specifics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderIntent {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    /// Positive quantity.
    pub qty: i64,
    /// Optional limit price in integer micros.
    pub limit_price_micros: Option<i64>,
    /// Optional stop price in integer micros.
    pub stop_price_micros: Option<i64>,
    /// Time-in-force string (e.g. "day", "gtc").
    pub time_in_force: String,
}

/// Execution intent is derived from order intents and risk rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIntent {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    /// Positive quantity.
    pub qty: i64,
    /// Optional limit price in integer micros.
    pub limit_price_micros: Option<i64>,
    /// Optional stop price in integer micros.
    pub stop_price_micros: Option<i64>,
    /// Time-in-force string (e.g. "day", "gtc").
    pub time_in_force: String,
}

/// Execution decision describes what the engine will do for this tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionDecision {
    Noop,
    PlaceOrders(Vec<ExecutionIntent>),
    HaltAndDisarm { reason: String },
}

// ---------------------------------------------------------------------------
// Forward-compatible multi-asset types (V2)
// ---------------------------------------------------------------------------

use mqk_schemas::{
    AssetClass, ContractSpec, Instrument, OptionRight, OrderSide as CanonSide, OrderSpec,
    OrderType as CanonType, QtyMicros,
};

/// Create a canonical equity instrument (USD, no venue).
///
/// This is a bridge helper so existing symbol-based flows can produce a stable
/// `Instrument` without changing the current execution engine contract.
pub fn equity_instrument<S: Into<String>>(symbol: S) -> Instrument {
    Instrument {
        symbol: symbol.into(),
        asset_class: AssetClass::Equity,
        venue: None,
        currency: "USD".to_string(),
        contract: ContractSpec::Equity,
    }
}

/// V2 order intent keyed by `Instrument` and fractional quantity support.
///
/// Note: `qty` is always positive; `side` encodes direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderIntentV2 {
    pub instrument_id: Option<String>,
    pub instrument: Instrument,
    pub contract: IntentV2Contract,
    pub side: CanonSide,
    pub qty: QtyMicros,
    pub order_type: CanonType,
    pub limit_price_micros: Option<i64>,
    pub stop_price_micros: Option<i64>,
    pub time_in_force: String,
    pub strategy_id: Option<String>,
    pub source: Option<String>,
    pub research_only: bool,
    /// Optional protective bracket/OCO legs. See [`BracketLegs`] — this is a
    /// pure model-level representation only; no execution path anywhere in
    /// the repo constructs, submits, or acts on bracket/OCO orders today.
    pub bracket: Option<BracketLegs>,
}

impl OrderIntentV2 {
    pub fn new(instrument: Instrument, side: CanonSide, qty: QtyMicros) -> Self {
        debug_assert!(qty.raw() > 0, "OrderIntentV2.qty must be > 0");
        let contract = IntentV2Contract::from_instrument(&instrument);
        Self {
            instrument_id: None,
            instrument,
            contract,
            side,
            qty,
            order_type: CanonType::Market,
            limit_price_micros: None,
            stop_price_micros: None,
            time_in_force: "DAY".to_string(),
            strategy_id: None,
            source: None,
            research_only: false,
            bracket: None,
        }
    }

    pub fn with_instrument_id<S: Into<String>>(mut self, instrument_id: S) -> Self {
        self.instrument_id = Some(instrument_id.into());
        self
    }

    pub fn with_contract(mut self, contract: IntentV2Contract) -> Self {
        self.contract = contract;
        self
    }

    pub fn with_order_type(mut self, order_type: CanonType) -> Self {
        self.order_type = order_type;
        self
    }

    pub fn with_limit_price_micros(mut self, limit_price_micros: i64) -> Self {
        self.limit_price_micros = Some(limit_price_micros);
        self
    }

    pub fn with_stop_price_micros(mut self, stop_price_micros: i64) -> Self {
        self.stop_price_micros = Some(stop_price_micros);
        self
    }

    pub fn with_time_in_force<S: Into<String>>(mut self, time_in_force: S) -> Self {
        self.time_in_force = time_in_force.into();
        self
    }

    pub fn with_strategy_source<S1: Into<String>, S2: Into<String>>(
        mut self,
        strategy_id: S1,
        source: S2,
    ) -> Self {
        self.strategy_id = Some(strategy_id.into());
        self.source = Some(source.into());
        self
    }

    pub fn as_research_only(mut self) -> Self {
        self.research_only = true;
        self
    }

    /// Attach protective bracket/OCO legs. Model-only: attaching this never
    /// makes the intent routable — see [`validate_order_intent_v2`], which
    /// always reports `DisabledAssetClass` for any intent carrying valid
    /// bracket legs, regardless of asset class, because no execution path
    /// anywhere supports multi-leg/bracket/OCO submission today.
    pub fn with_bracket_legs(mut self, bracket: BracketLegs) -> Self {
        self.bracket = Some(bracket);
        self
    }

    pub fn validate_model(&self) -> IntentV2Validation {
        validate_order_intent_v2(self)
    }

    pub fn validate_model_with_caller_routing_request(
        &self,
        _caller_routing_requested: bool,
    ) -> IntentV2Validation {
        validate_order_intent_v2(self)
    }

    /// Strict equity guard: requires whole-unit quantity.
    pub fn assert_equity_whole_units(&self) {
        if self.instrument.asset_class == AssetClass::Equity {
            debug_assert!(
                self.qty.is_whole(),
                "equity qty must be whole units (multiple of 1_000_000 micros)"
            );
        }
    }
}

/// Inert v2 contract details carried by [`OrderIntentV2`].
///
/// This deliberately stays local to the v2 scaffold. It is not a broker
/// adapter contract, and it is not consumed by the live/paper order path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentV2Contract {
    Equity {
        instrument_kind: Option<String>,
    },
    CryptoSpot {
        base_currency: String,
        quote_currency: String,
    },
    Future {
        root: String,
        expiry_yyyymm: String,
        multiplier: i32,
        tick_size_micros: i64,
    },
    Option {
        underlying: String,
        expiry_yyyymmdd: String,
        strike_micros: i64,
        right: OptionRight,
        multiplier: i32,
    },
    ForexPair {
        base_currency: String,
        quote_currency: String,
    },
}

impl IntentV2Contract {
    fn from_instrument(instrument: &Instrument) -> Self {
        match &instrument.contract {
            ContractSpec::Equity => Self::Equity {
                instrument_kind: None,
            },
            ContractSpec::Crypto => {
                if instrument.asset_class == AssetClass::Forex {
                    Self::ForexPair {
                        base_currency: String::new(),
                        quote_currency: String::new(),
                    }
                } else {
                    Self::CryptoSpot {
                        base_currency: String::new(),
                        quote_currency: instrument.currency.clone(),
                    }
                }
            }
            ContractSpec::Future {
                root,
                expiry_yyyymm,
                multiplier,
                tick_size_micros,
            } => Self::Future {
                root: root.clone(),
                expiry_yyyymm: expiry_yyyymm.clone(),
                multiplier: *multiplier,
                tick_size_micros: *tick_size_micros,
            },
            ContractSpec::Option {
                underlying,
                expiry_yyyymmdd,
                strike_micros,
                right,
                multiplier,
            } => Self::Option {
                underlying: underlying.clone(),
                expiry_yyyymmdd: expiry_yyyymmdd.clone(),
                strike_micros: *strike_micros,
                right: *right,
                multiplier: *multiplier,
            },
        }
    }
}

/// Protective bracket/OCO legs attached to a parent [`OrderIntentV2`].
///
/// This models the common "bracket order" shape (a parent fill triggers a
/// one-cancels-other pair of a take-profit limit and a stop-loss stop) as
/// pure data. It deliberately stays local to the v2 scaffold: it is not a
/// broker adapter contract, it is not consumed by `ExecutionIntentV2` /
/// `OrderSpec`, and no execution, OMS, or broker path constructs child
/// orders from it. At least one of `take_profit_limit_price_micros` /
/// `stop_loss_stop_price_micros` must be set for the parent intent to
/// validate structurally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketLegs {
    pub take_profit_limit_price_micros: Option<i64>,
    pub stop_loss_stop_price_micros: Option<i64>,
    /// True when the two legs are linked one-cancels-other. Documentary
    /// only today — both legs are always non-routable regardless of this
    /// flag, since no OCO submission path exists.
    pub oco: bool,
}

impl BracketLegs {
    pub fn new(
        take_profit_limit_price_micros: Option<i64>,
        stop_loss_stop_price_micros: Option<i64>,
    ) -> Self {
        Self {
            take_profit_limit_price_micros,
            stop_loss_stop_price_micros,
            oco: true,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IntentV2Routability {
    ResearchOnly,
    EquityRoutableCandidate,
    DisabledAssetClass,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentV2Validation {
    pub valid: bool,
    pub routability: IntentV2Routability,
    pub reason_code: &'static str,
    pub message: String,
}

impl IntentV2Validation {
    fn invalid(reason_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            valid: false,
            routability: IntentV2Routability::Invalid,
            reason_code,
            message: message.into(),
        }
    }

    fn valid(routability: IntentV2Routability, reason_code: &'static str) -> Self {
        Self {
            valid: true,
            routability,
            reason_code,
            message: match routability {
                IntentV2Routability::ResearchOnly => {
                    "OrderIntentV2 is structurally valid and marked research-only.".to_string()
                }
                IntentV2Routability::EquityRoutableCandidate => {
                    "OrderIntentV2 is a structurally valid equity model-level candidate; this helper does not route it.".to_string()
                }
                IntentV2Routability::DisabledAssetClass => {
                    "OrderIntentV2 is structurally valid, but this asset class is disabled for production routing.".to_string()
                }
                IntentV2Routability::Invalid => {
                    "OrderIntentV2 is structurally invalid.".to_string()
                }
            },
        }
    }
}

fn validate_order_intent_v2(intent: &OrderIntentV2) -> IntentV2Validation {
    if let Some(invalid) = validate_common_order_fields(
        &intent.instrument.symbol,
        &intent.instrument.currency,
        intent.qty,
        intent.order_type,
        intent.limit_price_micros,
        intent.stop_price_micros,
        &intent.time_in_force,
    ) {
        return invalid;
    }

    if let Some(invalid) = validate_intent_contract(intent) {
        return invalid;
    }

    if let Some(invalid) = validate_bracket_legs(intent.bracket.as_ref()) {
        return invalid;
    }
    if intent.bracket.is_some() {
        // Bracket/OCO submission has no execution path anywhere in the repo
        // today, regardless of asset class — the parent equity/ETF path
        // included. See `BracketLegs` doc comment.
        return IntentV2Validation::valid(
            IntentV2Routability::DisabledAssetClass,
            "bracket_oco_model_only_not_executable",
        );
    }

    if intent.instrument.asset_class != AssetClass::Equity {
        return IntentV2Validation::valid(
            IntentV2Routability::DisabledAssetClass,
            "asset_class_disabled",
        );
    }
    if intent.research_only {
        return IntentV2Validation::valid(IntentV2Routability::ResearchOnly, "research_only");
    }
    IntentV2Validation::valid(
        IntentV2Routability::EquityRoutableCandidate,
        "equity_model_candidate",
    )
}

fn validate_bracket_legs(bracket: Option<&BracketLegs>) -> Option<IntentV2Validation> {
    let bracket = bracket?;
    if bracket.take_profit_limit_price_micros.is_none()
        && bracket.stop_loss_stop_price_micros.is_none()
    {
        return Some(IntentV2Validation::invalid(
            "bracket_requires_at_least_one_leg",
            "OrderIntentV2 bracket legs require at least one of take-profit or stop-loss.",
        ));
    }
    if let Some(price) = bracket.take_profit_limit_price_micros {
        if price <= 0 {
            return Some(IntentV2Validation::invalid(
                "invalid_bracket_take_profit_price",
                "OrderIntentV2 bracket take-profit price must be positive.",
            ));
        }
    }
    if let Some(price) = bracket.stop_loss_stop_price_micros {
        if price <= 0 {
            return Some(IntentV2Validation::invalid(
                "invalid_bracket_stop_loss_price",
                "OrderIntentV2 bracket stop-loss price must be positive.",
            ));
        }
    }
    None
}

fn validate_order_spec_v2(spec: &OrderSpec) -> IntentV2Validation {
    if let Some(invalid) = validate_common_order_fields(
        &spec.instrument.symbol,
        &spec.instrument.currency,
        spec.qty,
        spec.order_type,
        spec.limit_price_micros,
        spec.stop_price_micros,
        &spec.time_in_force,
    ) {
        return invalid;
    }

    if let Some(invalid) = validate_order_spec_contract(spec) {
        return invalid;
    }

    if spec.instrument.asset_class != AssetClass::Equity {
        return IntentV2Validation::valid(
            IntentV2Routability::DisabledAssetClass,
            "asset_class_disabled",
        );
    }
    IntentV2Validation::valid(
        IntentV2Routability::EquityRoutableCandidate,
        "equity_model_candidate",
    )
}

fn validate_common_order_fields(
    symbol: &str,
    currency: &str,
    qty: QtyMicros,
    order_type: CanonType,
    limit_price_micros: Option<i64>,
    stop_price_micros: Option<i64>,
    time_in_force: &str,
) -> Option<IntentV2Validation> {
    if symbol.trim().is_empty() {
        return Some(IntentV2Validation::invalid(
            "missing_symbol",
            "OrderIntentV2 requires a non-empty symbol-like identity.",
        ));
    }
    if currency.trim().is_empty() {
        return Some(IntentV2Validation::invalid(
            "missing_currency",
            "OrderIntentV2 requires a non-empty settlement currency.",
        ));
    }
    if qty.raw() <= 0 {
        return Some(IntentV2Validation::invalid(
            "invalid_qty",
            "OrderIntentV2 quantity must be positive.",
        ));
    }
    if time_in_force.trim().is_empty() {
        return Some(IntentV2Validation::invalid(
            "missing_time_in_force",
            "OrderIntentV2 requires a non-empty time in force.",
        ));
    }

    match order_type {
        CanonType::Market => None,
        CanonType::Limit => validate_price("missing_limit_price", limit_price_micros),
        CanonType::Stop => validate_price("missing_stop_price", stop_price_micros),
        CanonType::StopLimit => validate_price("missing_stop_limit_price", limit_price_micros)
            .or_else(|| validate_price("missing_stop_limit_price", stop_price_micros)),
    }
}

fn validate_price(
    reason_code: &'static str,
    price_micros: Option<i64>,
) -> Option<IntentV2Validation> {
    match price_micros {
        Some(price) if price > 0 => None,
        _ => Some(IntentV2Validation::invalid(
            reason_code,
            "OrderIntentV2 order type requires a positive price in micros.",
        )),
    }
}

fn validate_intent_contract(intent: &OrderIntentV2) -> Option<IntentV2Validation> {
    match (
        &intent.instrument.asset_class,
        &intent.instrument.contract,
        &intent.contract,
    ) {
        (AssetClass::Equity, ContractSpec::Equity, IntentV2Contract::Equity { .. }) => None,
        (
            AssetClass::Crypto,
            ContractSpec::Crypto,
            IntentV2Contract::CryptoSpot {
                base_currency,
                quote_currency,
            },
        ) => validate_currency_pair(base_currency, quote_currency, "missing_crypto_pair"),
        (
            AssetClass::Future,
            ContractSpec::Future { .. },
            IntentV2Contract::Future {
                root,
                expiry_yyyymm,
                multiplier,
                tick_size_micros,
            },
        ) => validate_future_contract(root, expiry_yyyymm, *multiplier, *tick_size_micros),
        (
            AssetClass::Option,
            ContractSpec::Option { .. },
            IntentV2Contract::Option {
                underlying,
                expiry_yyyymmdd,
                strike_micros,
                multiplier,
                ..
            },
        ) => validate_option_contract(underlying, expiry_yyyymmdd, *strike_micros, *multiplier),
        (
            AssetClass::Forex,
            _,
            IntentV2Contract::ForexPair {
                base_currency,
                quote_currency,
            },
        ) => validate_currency_pair(base_currency, quote_currency, "missing_forex_pair"),
        _ => Some(IntentV2Validation::invalid(
            "asset_contract_mismatch",
            "OrderIntentV2 asset class and contract shape do not match.",
        )),
    }
}

fn validate_order_spec_contract(spec: &OrderSpec) -> Option<IntentV2Validation> {
    match (&spec.instrument.asset_class, &spec.instrument.contract) {
        (AssetClass::Equity, ContractSpec::Equity) => None,
        (AssetClass::Crypto, ContractSpec::Crypto) => None,
        (
            AssetClass::Future,
            ContractSpec::Future {
                root,
                expiry_yyyymm,
                multiplier,
                tick_size_micros,
            },
        ) => validate_future_contract(root, expiry_yyyymm, *multiplier, *tick_size_micros),
        (
            AssetClass::Option,
            ContractSpec::Option {
                underlying,
                expiry_yyyymmdd,
                strike_micros,
                multiplier,
                ..
            },
        ) => validate_option_contract(underlying, expiry_yyyymmdd, *strike_micros, *multiplier),
        _ => Some(IntentV2Validation::invalid(
            "asset_contract_mismatch",
            "ExecutionIntentV2 asset class and contract shape do not match.",
        )),
    }
}

fn validate_currency_pair(
    base_currency: &str,
    quote_currency: &str,
    reason_code: &'static str,
) -> Option<IntentV2Validation> {
    if base_currency.trim().is_empty() || quote_currency.trim().is_empty() {
        Some(IntentV2Validation::invalid(
            reason_code,
            "OrderIntentV2 currency-pair contract requires base and quote currencies.",
        ))
    } else {
        None
    }
}

fn validate_future_contract(
    root: &str,
    expiry_yyyymm: &str,
    multiplier: i32,
    tick_size_micros: i64,
) -> Option<IntentV2Validation> {
    if root.trim().is_empty()
        || expiry_yyyymm.trim().is_empty()
        || multiplier <= 0
        || tick_size_micros <= 0
    {
        Some(IntentV2Validation::invalid(
            "invalid_future_contract",
            "OrderIntentV2 future contract requires root, expiry, positive multiplier, and positive tick size.",
        ))
    } else {
        None
    }
}

fn validate_option_contract(
    underlying: &str,
    expiry_yyyymmdd: &str,
    strike_micros: i64,
    multiplier: i32,
) -> Option<IntentV2Validation> {
    if underlying.trim().is_empty()
        || expiry_yyyymmdd.trim().is_empty()
        || strike_micros <= 0
        || multiplier <= 0
    {
        Some(IntentV2Validation::invalid(
            "invalid_option_contract",
            "OrderIntentV2 option contract requires underlying, expiry, positive strike, and positive multiplier.",
        ))
    } else {
        None
    }
}

/// V2 execution intent aligns with `mqk-schemas::OrderSpec` for broker adapters.
///
/// This does NOT change the current execution pipeline yet; it simply provides
/// the shared contract needed to expand asset classes later without rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIntentV2 {
    pub spec: OrderSpec,
}

impl ExecutionIntentV2 {
    pub fn market(
        client_order_id: String,
        instrument: Instrument,
        side: CanonSide,
        qty: QtyMicros,
    ) -> Self {
        let spec = OrderSpec {
            client_order_id,
            instrument,
            side,
            order_type: CanonType::Market,
            qty,
            limit_price_micros: None,
            stop_price_micros: None,
            time_in_force: "DAY".to_string(),
        };
        Self { spec }
    }

    pub fn validate_model(&self) -> IntentV2Validation {
        validate_order_spec_v2(&self.spec)
    }
}
