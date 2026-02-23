use std::collections::BTreeMap;

/// 1e-6 fixed-point scale.
pub const MICROS_SCALE: i64 = 1_000_000;

/// Risk configuration (thresholds + policies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskConfig {
    /// If equity drops by this amount from day-start equity, halt trading.
    pub daily_loss_limit_micros: i64,

    /// If equity drops by this amount from peak equity, flatten + halt.
    pub max_drawdown_limit_micros: i64,

    /// If rejects in the current window reach this, halt (storm protection).
    pub reject_storm_max_rejects_in_window: u32,

    /// If true, enforce PDT: block new risk when pdt_ok == false.
    pub pdt_auto_enabled: bool,

    /// Missing protective stop: if true => FLATTEN+HALT and DISARM.
    pub missing_protective_stop_flattens: bool,
}

impl RiskConfig {
    pub fn sane_defaults() -> Self {
        Self {
            daily_loss_limit_micros: 0,
            max_drawdown_limit_micros: 0,
            reject_storm_max_rejects_in_window: 10,
            pdt_auto_enabled: true,
            missing_protective_stop_flattens: true,
        }
    }
}

/// What the caller is asking permission to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// Attempting to place a new order intent.
    NewOrder,
    /// Modify/cancel existing orders (not a new risk add).
    ModifyOrder,
    /// Flatten / reduce risk.
    Flatten,
}

/// PDT context (runtime provides this later; deterministic boolean in PATCH 07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdtContext {
    pub pdt_ok: bool,
}

impl PdtContext {
    pub fn ok() -> Self {
        Self { pdt_ok: true }
    }
    pub fn blocked() -> Self {
        Self { pdt_ok: false }
    }
}

/// Kill switch categories.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KillSwitchType {
    MissingProtectiveStop,
    StaleData,
    RejectStorm,
    Desync,
    Manual,
}

/// Kill switch event: code + evidence (deterministic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillSwitchEvent {
    pub kind: KillSwitchType,
    pub code: String,
    pub evidence: BTreeMap<String, String>,
}

impl KillSwitchEvent {
    pub fn new(kind: KillSwitchType) -> Self {
        // Deterministic code string
        let code = match kind {
            KillSwitchType::MissingProtectiveStop => "KILL_SWITCH_MISSING_PROTECTIVE_STOP",
            KillSwitchType::StaleData => "KILL_SWITCH_STALE_DATA",
            KillSwitchType::RejectStorm => "KILL_SWITCH_REJECT_STORM",
            KillSwitchType::Desync => "KILL_SWITCH_DESYNC",
            KillSwitchType::Manual => "KILL_SWITCH_MANUAL",
        }
        .to_string();

        Self {
            kind,
            code,
            evidence: BTreeMap::new(),
        }
    }

    pub fn with_evidence(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.evidence.insert(k.into(), v.into());
        self
    }
}

/// Inputs for one risk evaluation tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskInput {
    /// Deterministic trading day id (e.g., YYYYMMDD as int) provided by runtime.
    pub day_id: u32,

    /// Current equity in micros.
    pub equity_micros: i64,

    /// Deterministic reject window id (e.g., minute bucket counter) provided by runtime.
    pub reject_window_id: u32,

    /// Request context.
    pub request: RequestKind,

    /// True if this request reduces risk (closing / reducing / flatten).
    pub is_risk_reducing: bool,

    /// PDT context.
    pub pdt: PdtContext,

    /// Optional kill switch event (critical).
    pub kill_switch: Option<KillSwitchEvent>,
}

/// Risk engine output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskDecision {
    pub action: RiskAction,
    pub reason: ReasonCode,
    pub kill_switch: Option<KillSwitchEvent>,
}

/// Actions the risk engine can mandate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RiskAction {
    Allow,
    Reject,
    Halt,
    FlattenAndHalt,
}

/// Reason codes for decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReasonCode {
    Allowed,

    // Sticky regime
    AlreadyHalted,

    // Limits
    DailyLossLimitBreached,
    MaxDrawdownBreached,
    RejectStormBreached,

    // PDT
    PdtPrevented,

    // Kill switch
    KillSwitchTriggered,

    // Patch L10 — sanity clamps
    /// Input value failed basic sanity check (negative equity, zero/negative qty, overflow).
    BadInput,
}

/// Risk engine state (persisted by runtime later).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskState {
    pub day_id: u32,
    pub day_start_equity_micros: i64,
    pub peak_equity_micros: i64,

    pub halted: bool,
    pub disarmed: bool,

    pub reject_window_id: u32,
    pub reject_count_in_window: u32,
}

impl RiskState {
    pub fn new(day_id: u32, equity_micros: i64, reject_window_id: u32) -> Self {
        Self {
            day_id,
            day_start_equity_micros: equity_micros,
            peak_equity_micros: equity_micros,
            halted: false,
            disarmed: false,
            reject_window_id,
            reject_count_in_window: 0,
        }
    }

    /// Explicit deterministic reject recording (caller increments this when an order is rejected downstream).
    pub fn record_reject(&mut self, reject_window_id: u32) {
        if reject_window_id != self.reject_window_id {
            self.reject_window_id = reject_window_id;
            self.reject_count_in_window = 0;
        }
        self.reject_count_in_window = self.reject_count_in_window.saturating_add(1);
    }
}
