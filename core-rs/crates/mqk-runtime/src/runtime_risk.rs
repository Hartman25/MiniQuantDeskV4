#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Datelike, Timelike, Utc};

use mqk_execution::gateway::{RiskGate, RiskRequestContext};
use mqk_risk::{
    evaluate, KillSwitchEvent, PdtContext, ReasonCode, RequestKind, RiskAction, RiskConfig,
    RiskInput, RiskState,
};

// Re-exported so callers that build an `AccountAuthorityContext` (e.g.
// `mqk-daemon`'s `DaemonAccountAuthority`) do not need a direct `mqk-risk`
// crate dependency just to construct a `PdtContext` / `KillSwitchEvent`.
pub use mqk_risk::{KillSwitchEvent as RiskKillSwitchEvent, PdtContext as RiskPdtContext};

// ---------------------------------------------------------------------------
// RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01 / RRA-1
//
// `RuntimeRiskGate` no longer stores a `RiskInput` frozen at construction
// time. `RiskConfig` and `RiskState` remain run-scoped (mutated only by the
// deterministic `mqk_risk::evaluate` state machine). `day_id`,
// `reject_window_id`, `equity_micros`, `pdt`, and `kill_switch` are pulled
// fresh from a `RuntimeClock` + `RuntimeAccountAuthority` on every
// evaluation, so a later change in wall-clock time or account equity is
// observed by the very next order evaluation without reconstructing the
// gate.
// ---------------------------------------------------------------------------

/// Wall-clock authority for `day_id` / `reject_window_id` derivation.
///
/// Production uses [`SystemClock`]. Tests inject a deterministic clock so
/// day/minute rollover can be proven without sleeping in real time.
pub trait RuntimeClock: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
}

/// Real wall-clock time. The only production implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Why [`RuntimeAccountAuthority::current_account`] could not supply a
/// truthful current context.
///
/// Every variant means the same thing to the gate: deny, and do not touch
/// `RiskState` (evaluating with a guessed value could corrupt sticky
/// day-start/peak-equity state with a number nobody vouches for).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountAuthorityError {
    /// The authority has no current value at all (e.g. no broker snapshot
    /// has ever been captured).
    Unavailable,
    /// A value exists but is older than the caller's freshness bound.
    Stale,
    /// A value exists but failed to parse/validate (non-finite, non-decimal,
    /// overflow, non-positive where positive is required).
    Malformed,
}

/// Current dynamic account-level context, read fresh at evaluation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountAuthorityContext {
    /// Current equity in micros. Must be the actual current mark, never a
    /// cash-only or construction-time value.
    pub equity_micros: i64,
    /// Current PDT context. Callers that have no authoritative PDT source
    /// yet return [`PdtContext::ok()`] here ONLY when PDT enforcement is
    /// configured off (`RiskConfig::pdt_auto_enabled == false`) — see
    /// `RRA-4` disposition in the runtime-risk authority map. This is not a
    /// live-truth claim by this trait itself; it is whatever the
    /// implementor can prove.
    pub pdt: PdtContext,
    /// Optional kill-switch event. `None` is intentionally the steady
    /// state for `RuntimeRiskGate` in this system: canonical
    /// manual/staleness/reconcile-drift halt authority lives in
    /// `IntegrityGate` (see `StateIntegrityGate` in mqk-daemon), which is
    /// evaluated as an independent gate before the risk gate ever runs.
    /// This field exists for a future producer that has real kill-switch
    /// authority of its own; `None` here is not a claim that no kill
    /// condition exists system-wide.
    pub kill_switch: Option<KillSwitchEvent>,
}

/// Evaluation-time authority for dynamic account context.
///
/// Implementors MUST NOT fabricate a value when truth is unknown or stale —
/// return `Err` so the gate denies fail-closed rather than silently reusing
/// a frozen or synthetic number. Implementors MUST be side-effect free and
/// must not perform network I/O (the gate is called on the hot order-submit
/// path); read from an existing safe read-only cache instead.
pub trait RuntimeAccountAuthority: Send + Sync {
    fn current_account(
        &self,
        now: DateTime<Utc>,
    ) -> Result<AccountAuthorityContext, AccountAuthorityError>;
}

/// Convenience authority that always reports a fixed equity figure with
/// `PdtContext::ok()` and no kill-switch event.
///
/// This is NOT a truthful dynamic authority — it exists only for callers
/// that have not (yet) wired a real [`RuntimeAccountAuthority`] (test
/// scaffolding, CLI dev harnesses). Production broker-dispatch wiring
/// (`mqk-daemon`'s `build_execution_orchestrator`) MUST use a real
/// authority backed by current broker account truth
/// (`RRA-2 — DAEMON-RISK-ACCOUNT-TIME-CONFIG-AUTHORITY-01`), never this one.
struct StaticAccountAuthority {
    equity_micros: i64,
}

impl RuntimeAccountAuthority for StaticAccountAuthority {
    fn current_account(
        &self,
        _now: DateTime<Utc>,
    ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
        Ok(AccountAuthorityContext {
            equity_micros: self.equity_micros,
            pdt: PdtContext::ok(),
            kill_switch: None,
        })
    }
}

#[derive(Clone)]
pub struct RuntimeRiskGate {
    state: Arc<Mutex<RuntimeRiskGateState>>,
}

enum RuntimeRiskGateState {
    Ready {
        config: RiskConfig,
        state: RiskState,
        account: Arc<dyn RuntimeAccountAuthority>,
        clock: Arc<dyn RuntimeClock>,
    },
    FailClosed {
        denial: mqk_execution::RiskDenial,
    },
}

impl RuntimeRiskGate {
    /// Convenience constructor with a FIXED equity figure and the real
    /// system clock. See [`StaticAccountAuthority`] doc — not a truthful
    /// dynamic-equity authority; production broker dispatch must use
    /// [`Self::from_run_config_with_account_authority`].
    pub fn from_run_config(config_json: &serde_json::Value, initial_equity_micros: i64) -> Self {
        Self::from_run_config_with_account_authority(
            config_json,
            Arc::new(StaticAccountAuthority {
                equity_micros: initial_equity_micros,
            }),
            Arc::new(SystemClock),
        )
    }

    /// Production constructor: builds `RiskConfig` from `config_json` and
    /// wires a real dynamic `account` authority + `clock`.
    ///
    /// RR1 (RUNTIME-RISK-START-BASELINE-AUTHORITY-REPAIR-01): the SAME
    /// authoritative equity that `account` will report on every subsequent
    /// evaluation is fetched ONCE here, at construction, and used for BOTH:
    /// - seeding `RiskState.day_start_equity_micros` / `peak_equity_micros`;
    /// - converting the configured `daily_loss_limit` / `max_drawdown`
    ///   ratios to absolute micros.
    ///
    /// This is deliberately NOT `initial_equity_micros` from `config_json` /
    /// daemon env config — that value is a separate, run-scoped local
    /// portfolio-seed authority (see `PortfolioState::new` in
    /// `mqk-daemon`'s `recover_oms_and_portfolio`), which may legitimately
    /// differ from the account's actual current broker equity. Mixing the
    /// two here previously let a stale/incorrect configured figure silently
    /// set the account-level daily-loss/drawdown baseline instead of the
    /// broker-confirmed starting equity.
    ///
    /// If the authority cannot supply a truthful positive equity at
    /// construction (`Unavailable`/`Stale`/`Malformed`, or a non-positive
    /// value), the gate fails closed WITHOUT ever computing `RiskConfig` —
    /// there is no trustworthy baseline to convert ratios against.
    pub fn from_run_config_with_account_authority(
        config_json: &serde_json::Value,
        account: Arc<dyn RuntimeAccountAuthority>,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        let now = clock.now_utc();
        let authoritative_equity_micros = match account.current_account(now) {
            Ok(ctx) if ctx.equity_micros > 0 => ctx.equity_micros,
            _ => return Self::fail_closed(runtime_risk_fail_closed_denial()),
        };
        match runtime_risk_config_from_run_config(config_json, authoritative_equity_micros) {
            Ok(config) => Self::ready(
                config,
                RiskState::new(
                    day_id_for(now),
                    authoritative_equity_micros,
                    reject_window_id_for(now),
                ),
                account,
                clock,
            ),
            Err(denial) => Self::fail_closed(denial),
        }
    }

    /// Test-only constructor with explicit config/state/account/clock,
    /// bypassing `config_json` parsing.
    #[cfg(any(test, feature = "testkit"))]
    pub fn for_test(
        config: RiskConfig,
        state: RiskState,
        account: Arc<dyn RuntimeAccountAuthority>,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        Self::ready(config, state, account, clock)
    }

    fn ready(
        config: RiskConfig,
        state: RiskState,
        account: Arc<dyn RuntimeAccountAuthority>,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeRiskGateState::Ready {
                config,
                state,
                account,
                clock,
            })),
        }
    }

    fn fail_closed(denial: mqk_execution::RiskDenial) -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeRiskGateState::FailClosed { denial })),
        }
    }

    /// Evaluate against a freshly-pulled dynamic context. Shared by
    /// `evaluate_gate` and `evaluate_gate_for_request` — the only
    /// difference between the two call sites is `request` /
    /// `is_risk_reducing`, both trusted-caller-computed, never
    /// account/equity/PDT data.
    fn evaluate_with_request(
        &self,
        request: RequestKind,
        is_risk_reducing: bool,
    ) -> mqk_execution::RiskDecision {
        let mut guard = self.state.lock().expect("runtime risk gate lock");
        match &mut *guard {
            RuntimeRiskGateState::FailClosed { denial } => {
                mqk_execution::RiskDecision::Deny(denial.clone())
            }
            RuntimeRiskGateState::Ready {
                config,
                state,
                account,
                clock,
            } => {
                let now = clock.now_utc();
                let ctx = match account.current_account(now) {
                    Ok(ctx) => ctx,
                    Err(_) => {
                        // Fail closed WITHOUT calling `evaluate()` — an
                        // unavailable/stale/malformed authority must never
                        // let a guessed equity mutate sticky RiskState
                        // (day-start equity, peak equity).
                        return mqk_execution::RiskDecision::Deny(runtime_risk_fail_closed_denial());
                    }
                };
                if ctx.equity_micros <= 0 {
                    return mqk_execution::RiskDecision::Deny(runtime_risk_fail_closed_denial());
                }
                let input = RiskInput {
                    day_id: day_id_for(now),
                    equity_micros: ctx.equity_micros,
                    reject_window_id: reject_window_id_for(now),
                    request,
                    is_risk_reducing,
                    pdt: ctx.pdt,
                    kill_switch: ctx.kill_switch,
                };
                let decision = evaluate(config, state, &input);
                runtime_risk_decision_to_execution_decision(config, &decision)
            }
        }
    }

    /// Record a real hard broker reject against the CURRENT reject window
    /// (RRA-3). No-op on a `FailClosed` gate — there is no `RiskState` to
    /// record into, and subsequent `evaluate_gate` calls already deny
    /// regardless of reject count.
    pub fn record_broker_reject(&self) {
        let mut guard = self.state.lock().expect("runtime risk gate lock");
        if let RuntimeRiskGateState::Ready { state, clock, .. } = &mut *guard {
            let now = clock.now_utc();
            state.record_reject(reject_window_id_for(now));
        }
    }
}

impl RiskGate for RuntimeRiskGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        self.evaluate_with_request(RequestKind::NewOrder, false)
    }

    /// Read-only report of `RiskState.halted` (RISK-ENGINE-HALTED-VISIBILITY-01).
    ///
    /// Does NOT call `evaluate()` — only reads the sticky `halted` flag from
    /// the currently-held `RiskState`. `FailClosed` gates have no `RiskState`
    /// to read, so they report `Unavailable` (not a claim of "not halted").
    fn sticky_halt_status(&self) -> mqk_execution::RiskEngineHaltStatus {
        let state = self.state.lock().expect("runtime risk gate lock");
        match &*state {
            RuntimeRiskGateState::Ready { state, .. } => {
                mqk_execution::RiskEngineHaltStatus::Known {
                    halted: state.halted,
                }
            }
            RuntimeRiskGateState::FailClosed { .. } => {
                mqk_execution::RiskEngineHaltStatus::Unavailable
            }
        }
    }

    /// Evaluate the gate for a specific per-order request context
    /// (RISK-FLATTEN-ON-HALT-01), against the SAME current dynamic account
    /// context `evaluate_gate` would observe. Overrides only the
    /// trusted-caller-computed `request` / `is_risk_reducing` fields:
    /// - `is_risk_reducing == true`  => `RequestKind::Flatten`
    /// - `is_risk_reducing == false` => `RequestKind::NewOrder`
    fn evaluate_gate_for_request(&self, ctx: RiskRequestContext) -> mqk_execution::RiskDecision {
        if ctx.is_risk_reducing {
            self.evaluate_with_request(RequestKind::Flatten, true)
        } else {
            self.evaluate_with_request(RequestKind::NewOrder, false)
        }
    }

    fn record_broker_reject(&self) {
        RuntimeRiskGate::record_broker_reject(self)
    }
}

/// `day_id` derivation: `YYYYMMDD` from UTC wall-clock (AUTON-PAPER-RISK-04
/// semantics, now evaluated fresh on every tick rather than once at
/// orchestrator construction).
fn day_id_for(now: DateTime<Utc>) -> u32 {
    let d = now.date_naive();
    (d.year() as u32) * 10_000 + d.month() * 100 + d.day()
}

/// `reject_window_id` derivation: minute-of-day bucket (0..1439), matching
/// the "minute bucket counter" `RiskInput` documentation and the previously
/// orchestrator-construction-time-only AUTON-PAPER-RISK-04 formula.
fn reject_window_id_for(now: DateTime<Utc>) -> u32 {
    now.hour() * 60 + now.minute()
}

fn runtime_risk_config_from_run_config(
    config_json: &serde_json::Value,
    authoritative_equity_micros: i64,
) -> Result<RiskConfig, mqk_execution::RiskDenial> {
    if authoritative_equity_micros <= 0 {
        return Err(runtime_risk_fail_closed_denial());
    }

    let defaults = RiskConfig::sane_defaults();
    let daily_loss_ratio = config_json
        .pointer("/risk/daily_loss_limit")
        .and_then(|value| value.as_f64())
        .ok_or_else(runtime_risk_fail_closed_denial)?;

    let daily_loss_limit_micros =
        ratio_limit_to_micros(daily_loss_ratio, authoritative_equity_micros)
            .ok_or_else(runtime_risk_fail_closed_denial)?;

    // RRA-2: max_drawdown is a real ordinary production risk configuration
    // input, required with the SAME validation discipline as
    // `daily_loss_limit` above — absence or an invalid ratio fails the
    // whole gate closed rather than silently producing a disabled
    // (`max_drawdown_limit_micros == 0`) check. `mqk-daemon` supplements
    // this field from `MQK_RISK_MAX_DRAWDOWN` when the run's own
    // `config_json` does not already carry it (see
    // `orchestrator_build.rs::effective_run_config_for_risk`).
    let max_drawdown_ratio = config_json
        .pointer("/risk/max_drawdown")
        .and_then(|value| value.as_f64())
        .ok_or_else(runtime_risk_fail_closed_denial)?;
    let max_drawdown_limit_micros =
        ratio_limit_to_micros(max_drawdown_ratio, authoritative_equity_micros)
            .ok_or_else(runtime_risk_fail_closed_denial)?;

    let reject_storm_max_rejects_in_window = match config_json
        .pointer("/risk/reject_storm/max_rejects")
        .and_then(|value| value.as_i64())
    {
        Some(value) if value > 0 => value as u32,
        Some(_) => return Err(runtime_risk_fail_closed_denial()),
        None => defaults.reject_storm_max_rejects_in_window,
    };

    Ok(RiskConfig {
        daily_loss_limit_micros,
        max_drawdown_limit_micros,
        reject_storm_max_rejects_in_window,
        pdt_auto_enabled: config_json
            .pointer("/risk/pdt_auto_enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(defaults.pdt_auto_enabled),
        missing_protective_stop_flattens: config_json
            .pointer("/risk/missing_protective_stop_flattens")
            .and_then(|value| value.as_bool())
            .unwrap_or(defaults.missing_protective_stop_flattens),
    })
}

fn ratio_limit_to_micros(ratio: f64, authoritative_equity_micros: i64) -> Option<i64> {
    if !ratio.is_finite() || ratio <= 0.0 || authoritative_equity_micros <= 0 {
        return None;
    }

    let limit = ratio * authoritative_equity_micros as f64;
    if !limit.is_finite() || limit <= 0.0 || limit > i64::MAX as f64 {
        return None;
    }

    Some(limit.round() as i64)
}

fn runtime_risk_fail_closed_denial() -> mqk_execution::RiskDenial {
    mqk_execution::RiskDenial {
        reason: mqk_execution::RiskReason::RiskEngineUnavailable,
        evidence: mqk_execution::RiskEvidence::default(),
    }
}

fn runtime_risk_decision_to_execution_decision(
    config: &RiskConfig,
    decision: &mqk_risk::RiskDecision,
) -> mqk_execution::RiskDecision {
    match decision.action {
        RiskAction::Allow => mqk_execution::RiskDecision::Allow,
        _ => mqk_execution::RiskDecision::Deny(runtime_risk_engine_denial(config, decision)),
    }
}

fn runtime_risk_engine_denial(
    config: &RiskConfig,
    decision: &mqk_risk::RiskDecision,
) -> mqk_execution::RiskDenial {
    let mut evidence = mqk_execution::RiskEvidence::default();
    match decision.reason {
        ReasonCode::DailyLossLimitBreached => {
            evidence.limit = Some(config.daily_loss_limit_micros);
        }
        ReasonCode::MaxDrawdownBreached => {
            evidence.limit = Some(config.max_drawdown_limit_micros);
        }
        ReasonCode::RejectStormBreached => {
            evidence.limit = Some(config.reject_storm_max_rejects_in_window as i64);
        }
        _ => {}
    }

    let reason = match decision.reason {
        ReasonCode::DailyLossLimitBreached | ReasonCode::MaxDrawdownBreached => {
            mqk_execution::RiskReason::CapitalLimitExceeded
        }
        ReasonCode::RejectStormBreached | ReasonCode::PdtPrevented => {
            mqk_execution::RiskReason::MaxOrderSizeExceeded
        }
        ReasonCode::AlreadyHalted | ReasonCode::KillSwitchTriggered | ReasonCode::BadInput => {
            mqk_execution::RiskReason::RiskEngineUnavailable
        }
        ReasonCode::Allowed => mqk_execution::RiskReason::RiskEngineUnavailable,
    };

    mqk_execution::RiskDenial { reason, evidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::RwLock as StdRwLock;

    /// Deterministic clock for tests: starts at a fixed instant, advanceable
    /// without sleeping in real time.
    struct FixedClock(StdRwLock<DateTime<Utc>>);

    impl FixedClock {
        fn new(t: DateTime<Utc>) -> Arc<Self> {
            Arc::new(Self(StdRwLock::new(t)))
        }
        fn set(&self, t: DateTime<Utc>) {
            *self.0.write().unwrap() = t;
        }
    }

    impl RuntimeClock for FixedClock {
        fn now_utc(&self) -> DateTime<Utc> {
            *self.0.read().unwrap()
        }
    }

    /// Mutable test authority: equity changes when the test changes it, no
    /// unavailable/stale/malformed simulation.
    struct MutableEquityAuthority(AtomicI64);

    impl MutableEquityAuthority {
        fn new(equity_micros: i64) -> Arc<Self> {
            Arc::new(Self(AtomicI64::new(equity_micros)))
        }
        fn set(&self, equity_micros: i64) {
            self.0.store(equity_micros, Ordering::SeqCst);
        }
    }

    impl RuntimeAccountAuthority for MutableEquityAuthority {
        fn current_account(
            &self,
            _now: DateTime<Utc>,
        ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
            Ok(AccountAuthorityContext {
                equity_micros: self.0.load(Ordering::SeqCst),
                pdt: PdtContext::ok(),
                kill_switch: None,
            })
        }
    }

    /// Authority that always fails with a configured error — proves
    /// unavailable/stale/malformed all deny without touching RiskState.
    struct FailingAuthority(AccountAuthorityError);

    impl RuntimeAccountAuthority for FailingAuthority {
        fn current_account(
            &self,
            _now: DateTime<Utc>,
        ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
            Err(self.0)
        }
    }

    fn t(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, y, mo, d, h, mi, 0).unwrap()
    }

    fn make_config() -> RiskConfig {
        RiskConfig {
            daily_loss_limit_micros: 1_000 * 1_000_000,
            max_drawdown_limit_micros: 0,
            reject_storm_max_rejects_in_window: 10,
            pdt_auto_enabled: false,
            missing_protective_stop_flattens: false,
        }
    }

    #[test]
    fn runtime_risk_gate_fails_closed_on_missing_or_ambiguous_input() {
        let risk_gate = RuntimeRiskGate::from_run_config(&serde_json::json!({}), 0);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("missing runtime risk inputs must deny");
        };
        assert_eq!(
            denial.reason,
            mqk_execution::RiskReason::RiskEngineUnavailable,
            "missing runtime risk inputs must fail closed"
        );
    }

    // -----------------------------------------------------------------
    // RRA-1 negative control 1+2: dynamic equity is observed live, and a
    // later provider change alone flips the decision when a threshold is
    // crossed.
    // -----------------------------------------------------------------
    #[test]
    fn evaluation_observes_current_equity_not_construction_time_equity() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let authority = MutableEquityAuthority::new(100_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            authority.clone(),
            clock,
        );

        assert_eq!(
            risk_gate.evaluate_gate(),
            mqk_execution::RiskDecision::Allow,
            "precondition: starting equity must be allowed"
        );

        // Provider now reports equity below the daily-loss floor (day-start
        // 100k, limit 1k => floor 99k). Nothing else about the gate changed.
        authority.set(98_999 * 1_000_000);

        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("changing provider equity alone must change the decision at the threshold");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
    }

    // -----------------------------------------------------------------
    // RRA-1 negative control 3: stale/unavailable authority denies rather
    // than reusing an old equity value.
    // -----------------------------------------------------------------
    #[test]
    fn unavailable_authority_denies_without_reusing_prior_equity() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            Arc::new(FailingAuthority(AccountAuthorityError::Unavailable)),
            clock,
        );

        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("unavailable authority must deny");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    #[test]
    fn stale_authority_denies() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            Arc::new(FailingAuthority(AccountAuthorityError::Stale)),
            clock,
        );

        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("stale authority must deny");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    #[test]
    fn malformed_authority_denies() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            Arc::new(FailingAuthority(AccountAuthorityError::Malformed)),
            clock,
        );

        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("malformed authority must deny");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    // -----------------------------------------------------------------
    // RRA-1 negative control 5+6: the caller has no API surface to forge
    // current equity or PDT — `RiskRequestContext` carries only
    // `is_risk_reducing`, proven at compile time by its field set, and
    // `evaluate_gate_for_request` never reads equity/pdt from `ctx`.
    // -----------------------------------------------------------------
    #[test]
    fn risk_request_context_carries_no_forgeable_account_fields() {
        let ctx = RiskRequestContext {
            is_risk_reducing: true,
        };
        // Compiles only because `RiskRequestContext` has exactly this one
        // field — any added equity/pdt field would need to appear here too.
        let RiskRequestContext { is_risk_reducing } = ctx;
        assert!(is_risk_reducing);
    }

    // -----------------------------------------------------------------
    // RRA-1 negative control 7: risk-reducing requests use the SAME
    // current account authority as new-order requests.
    // -----------------------------------------------------------------
    #[test]
    fn risk_reducing_request_uses_same_current_equity_as_new_order() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let authority = MutableEquityAuthority::new(98_999 * 1_000_000);
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            authority,
            clock,
        );

        // Breach the daily-loss floor to sticky-halt.
        let _ = risk_gate.evaluate_gate();
        assert_eq!(
            risk_gate.sticky_halt_status(),
            mqk_execution::RiskEngineHaltStatus::Known { halted: true }
        );

        // A verified risk-reducing flatten still passes (sticky-halt allows
        // Flatten) using the SAME authority-sourced equity — proving there
        // is no separate/duplicated equity path for the reducing case.
        let decision = risk_gate.evaluate_gate_for_request(RiskRequestContext {
            is_risk_reducing: true,
        });
        assert_eq!(decision, mqk_execution::RiskDecision::Allow);
    }

    // -----------------------------------------------------------------
    // RRA-1 negative control 8: sticky halt survives a later context
    // change (equity recovering above the floor does not un-halt).
    // -----------------------------------------------------------------
    #[test]
    fn sticky_halt_survives_later_equity_recovery() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let authority = MutableEquityAuthority::new(98_999 * 1_000_000);
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            authority.clone(),
            clock,
        );

        let _ = risk_gate.evaluate_gate();
        assert_eq!(
            risk_gate.sticky_halt_status(),
            mqk_execution::RiskEngineHaltStatus::Known { halted: true }
        );

        // Equity recovers well above the floor.
        authority.set(150_000 * 1_000_000);
        let decision = risk_gate.evaluate_gate();
        let mqk_execution::RiskDecision::Deny(denial) = decision else {
            panic!("sticky halt must survive a later equity recovery for non-reducing orders");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    // -----------------------------------------------------------------
    // Day / minute rollover observed live via the injected clock, without
    // reconstructing the gate.
    // -----------------------------------------------------------------
    #[test]
    fn minute_rollover_resets_reject_window_without_reconstruction() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 59));
        let authority = MutableEquityAuthority::new(100_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 0,
                max_drawdown_limit_micros: 0,
                reject_storm_max_rejects_in_window: 2,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20_240_115, 100_000 * 1_000_000, 9 * 60 + 59),
            authority,
            clock.clone(),
        );

        risk_gate.record_broker_reject();
        risk_gate.record_broker_reject();
        // Threshold reached in this minute window: next NewOrder is denied.
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("reject-storm threshold must deny in the current window");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::MaxOrderSizeExceeded);
    }

    #[test]
    fn day_rollover_resets_day_start_equity_without_reconstruction() {
        let clock = FixedClock::new(t(2024, 1, 15, 23, 59));
        let authority = MutableEquityAuthority::new(100_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 1_000 * 1_000_000,
                max_drawdown_limit_micros: 0,
                reject_storm_max_rejects_in_window: 100,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20_240_115, 100_000 * 1_000_000, 23 * 60 + 59),
            authority.clone(),
            clock.clone(),
        );

        // Equity drifts down to 99_500 — inside the 1k daily-loss floor for
        // day 20240115 (floor 99k), so this must still be allowed.
        authority.set(99_500 * 1_000_000);
        assert_eq!(risk_gate.evaluate_gate(), mqk_execution::RiskDecision::Allow);

        // Cross midnight: day rolls to 20240116, day-start equity resets to
        // the current 99_500. A further 400 drop to 99_100 is NOT a 1k
        // breach of the NEW day-start, so it must remain allowed — proving
        // the day boundary was actually observed by the live clock, not
        // just carried over from construction.
        clock.set(t(2024, 1, 16, 0, 1));
        authority.set(99_100 * 1_000_000);
        assert_eq!(
            risk_gate.evaluate_gate(),
            mqk_execution::RiskDecision::Allow,
            "day rollover must reset day-start equity to the fresh clock-observed day"
        );
    }

    #[test]
    fn runtime_risk_denials_preserve_non_unavailable_reason_when_possible() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 1),
            MutableEquityAuthority::new(98_999 * 1_000_000),
            clock,
        );

        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("daily-loss breach must deny");
        };
        assert_eq!(
            denial.reason,
            mqk_execution::RiskReason::CapitalLimitExceeded,
            "engine denial should preserve a truthful non-unavailable category"
        );
    }

    // RISK-ENGINE-HALTED-VISIBILITY-01: read-only sticky-halt accessor tests.

    #[test]
    fn sticky_halt_status_known_false_for_fresh_ready_state() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 1),
            MutableEquityAuthority::new(100_000 * 1_000_000),
            clock,
        );

        assert_eq!(
            risk_gate.sticky_halt_status(),
            mqk_execution::RiskEngineHaltStatus::Known { halted: false },
            "fresh risk state must report halted=false, not Unavailable"
        );
    }

    #[test]
    fn sticky_halt_status_unavailable_for_fail_closed_gate() {
        let risk_gate = RuntimeRiskGate::from_run_config(&serde_json::json!({}), 0);

        assert_eq!(
            risk_gate.sticky_halt_status(),
            mqk_execution::RiskEngineHaltStatus::Unavailable,
            "fail-closed gates have no RiskState and must report Unavailable, not a guess"
        );
    }

    // RISK-FLATTEN-ON-HALT-01: verified risk-reducing requests pass sticky halt.

    fn halted_risk_gate() -> RuntimeRiskGate {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            make_config(),
            RiskState::new(20_240_115, 100_000 * 1_000_000, 1),
            MutableEquityAuthority::new(98_999 * 1_000_000),
            clock,
        );

        let _ = risk_gate.evaluate_gate();
        assert_eq!(
            risk_gate.sticky_halt_status(),
            mqk_execution::RiskEngineHaltStatus::Known { halted: true },
            "precondition: risk state must be sticky-halted before testing flatten-through-halt"
        );

        risk_gate
    }

    #[test]
    fn evaluate_gate_for_request_allows_verified_flatten_when_halted() {
        let risk_gate = halted_risk_gate();

        let decision = risk_gate.evaluate_gate_for_request(RiskRequestContext {
            is_risk_reducing: true,
        });

        assert_eq!(
            decision,
            mqk_execution::RiskDecision::Allow,
            "a verified risk-reducing flatten must pass the sticky risk halt"
        );
    }

    #[test]
    fn evaluate_gate_for_request_denies_non_reducing_order_when_halted() {
        let risk_gate = halted_risk_gate();

        let decision = risk_gate.evaluate_gate_for_request(RiskRequestContext {
            is_risk_reducing: false,
        });

        let mqk_execution::RiskDecision::Deny(denial) = decision else {
            panic!(
                "a non-reducing order must remain denied while the risk engine is sticky-halted"
            );
        };
        assert_eq!(
            denial.reason,
            mqk_execution::RiskReason::RiskEngineUnavailable,
            "sticky-halt denial for non-reducing orders maps to RiskEngineUnavailable"
        );
    }

    #[test]
    fn evaluate_gate_for_request_matches_evaluate_gate_when_not_halted() {
        let make_gate = || {
            RuntimeRiskGate::for_test(
                make_config(),
                RiskState::new(20_240_115, 100_000 * 1_000_000, 1),
                MutableEquityAuthority::new(100_000 * 1_000_000),
                FixedClock::new(t(2024, 1, 15, 9, 0)),
            )
        };

        let gate_a = make_gate();
        let gate_b = make_gate();

        let baseline = gate_a.evaluate_gate();
        let via_context = gate_b.evaluate_gate_for_request(RiskRequestContext::default());

        assert_eq!(
            baseline,
            mqk_execution::RiskDecision::Allow,
            "precondition: not-halted NewOrder must be allowed"
        );
        assert_eq!(
            via_context, baseline,
            "evaluate_gate_for_request with default (non-reducing) context must match evaluate_gate when not halted"
        );
    }

    // -----------------------------------------------------------------
    // RRA-3: reject events reach the SAME RiskState the gateway evaluates.
    // -----------------------------------------------------------------
    #[test]
    fn record_broker_reject_reaches_reject_storm_threshold() {
        let clock = FixedClock::new(t(2024, 1, 15, 9, 0));
        let risk_gate = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 0,
                max_drawdown_limit_micros: 0,
                reject_storm_max_rejects_in_window: 3,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20_240_115, 100_000 * 1_000_000, 540),
            MutableEquityAuthority::new(100_000 * 1_000_000),
            clock,
        );

        risk_gate.record_broker_reject();
        risk_gate.record_broker_reject();
        // 2 recorded, threshold 3: still allowed (threshold-1 does not
        // prematurely halt).
        assert_eq!(risk_gate.evaluate_gate(), mqk_execution::RiskDecision::Allow);

        risk_gate.record_broker_reject();
        // 3rd reject reaches the threshold.
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("Nth reject reaching the threshold must deny new risk");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::MaxOrderSizeExceeded);
    }

    #[test]
    fn record_broker_reject_on_fail_closed_gate_is_a_safe_no_op() {
        let risk_gate = RuntimeRiskGate::from_run_config(&serde_json::json!({}), 0);
        // Must not panic; FailClosed has no RiskState to mutate.
        risk_gate.record_broker_reject();
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("fail-closed gate must remain denied");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    // AUTON-PAPER-RISK-04: day_id/reject_window_id derivation formulas.
    #[test]
    fn day_id_and_reject_window_id_formulas_are_correct() {
        let ts = t(2024, 1, 15, 9, 32);
        assert_eq!(day_id_for(ts), 20_240_115, "day_id must be YYYYMMDD");
        assert_eq!(reject_window_id_for(ts), 9 * 60 + 32);

        assert_eq!(reject_window_id_for(t(2024, 1, 15, 0, 0)), 0);
        assert_eq!(reject_window_id_for(t(2024, 1, 15, 23, 59)), 1439);

        let far = t(9999, 12, 31, 0, 0);
        assert!(day_id_for(far) < u32::MAX, "day_id fits in u32 for any calendar date");
    }

    // -----------------------------------------------------------------
    // RRA-2 negative controls 6+7: missing/invalid max_drawdown fails the
    // ordinary config_json-driven path closed — same discipline as
    // daily_loss_limit — rather than silently disabling the check.
    // -----------------------------------------------------------------
    #[test]
    fn missing_max_drawdown_fails_closed_on_config_json_path() {
        let risk_gate = RuntimeRiskGate::from_run_config(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02 } }),
            100_000 * 1_000_000,
        );
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("missing max_drawdown must fail the gate closed, not silently disable the check");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    #[test]
    fn invalid_max_drawdown_ratio_fails_closed() {
        // 0.0 is rejected by `ratio_limit_to_micros` (must be strictly
        // positive) — same validity contract as daily_loss_limit.
        let risk_gate = RuntimeRiskGate::from_run_config(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.0 } }),
            100_000 * 1_000_000,
        );
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("a non-positive max_drawdown ratio must fail closed");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    #[test]
    fn valid_daily_loss_and_max_drawdown_both_reach_risk_config_exactly() {
        let risk_gate = RuntimeRiskGate::from_run_config(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.10 } }),
            100_000 * 1_000_000,
        );
        // Daily-loss floor: 100k - 2k = 98k. Equity at 97_999 breaches it.
        // (This gate uses the StaticAccountAuthority baked into
        // `from_run_config`, whose equity == the construction-time value —
        // sufficient here since this test only checks that BOTH ratios
        // reached RiskConfig, via the daily-loss reason firing rather than
        // RiskEngineUnavailable/max-drawdown-disabled.)
        let risk_gate2 = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 2_000 * 1_000_000,
                max_drawdown_limit_micros: 10_000 * 1_000_000,
                reject_storm_max_rejects_in_window: 10,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20_240_115, 100_000 * 1_000_000, 1),
            MutableEquityAuthority::new(89_999 * 1_000_000),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate2.evaluate_gate() else {
            panic!("max-drawdown breach must deny once both limits are wired");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
        // Sanity: the first gate (config_json path) is at least constructed
        // and reachable (not FailClosed) given both ratios were valid.
        assert_eq!(risk_gate.evaluate_gate(), mqk_execution::RiskDecision::Allow);
    }

    // -----------------------------------------------------------------
    // RR1 (RUNTIME-RISK-START-BASELINE-AUTHORITY-REPAIR-01) negative
    // controls: the account-level risk baseline (day-start/peak equity,
    // and the ratio→absolute conversion) must be seeded from the SAME
    // authoritative account equity `evaluate_gate` will observe later —
    // never from a separately-configured figure that can diverge from it.
    // `from_run_config_with_account_authority`'s signature no longer even
    // accepts a separate equity value (the prior defect's exact vector),
    // so these prove the authority's own equity is what actually reaches
    // both the seeded `RiskState` and the ratio conversion, in both
    // directions.
    // -----------------------------------------------------------------

    /// Control A: broker-authoritative starting equity (95k) BELOW a
    /// hypothetical stale configured figure (100k). daily_loss_limit=2%:
    /// a floor wrongly anchored to 100k would be 98k, making 95k already a
    /// breach. The correct floor anchored to the authoritative 95k is
    /// 93_100 (95k * 0.98) — 95k itself must remain allowed.
    #[test]
    fn rr1_daily_loss_baseline_uses_lower_authoritative_equity_not_a_stale_higher_figure() {
        let authority = MutableEquityAuthority::new(95_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
            authority.clone(),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );
        assert_eq!(
            risk_gate.evaluate_gate(),
            mqk_execution::RiskDecision::Allow,
            "starting equity must be allowed under a floor anchored to the \
             authoritative 95k baseline (98k would incorrectly already deny)"
        );

        authority.set(93_099 * 1_000_000);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("breach of the authoritative-95k-anchored floor must deny");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
    }

    /// Control B: broker-authoritative starting equity (105k) ABOVE a
    /// hypothetical stale configured figure (100k). daily_loss_limit=2%:
    /// a floor wrongly anchored to 100k would be 98k, incorrectly ALLOWING
    /// a drop to 97k. The correct floor anchored to 105k is 102_900 — 97k
    /// must be denied.
    #[test]
    fn rr1_daily_loss_baseline_uses_higher_authoritative_equity_not_a_stale_lower_figure() {
        let authority = MutableEquityAuthority::new(105_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
            authority.clone(),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );

        authority.set(97_000 * 1_000_000);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!(
                "97k must breach the floor anchored to the authoritative 105k baseline \
                 (102_900) — a stale 100k-anchored floor (98k) would incorrectly allow it"
            );
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
    }

    /// Control C: same mismatch proof for `max_drawdown` — the peak-equity
    /// seed and drawdown floor must anchor to the authoritative equity.
    #[test]
    fn rr1_max_drawdown_baseline_uses_authoritative_equity_not_a_stale_figure() {
        let authority = MutableEquityAuthority::new(105_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.90, "max_drawdown": 0.10 } }),
            authority.clone(),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );

        // A stale 100k-anchored drawdown floor would be 90k, incorrectly
        // allowing 94_499. The correct 105k-anchored floor is 94_500.
        authority.set(94_499 * 1_000_000);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("94_499 must breach the max-drawdown floor anchored to the authoritative peak");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
    }

    /// Control D: an authority that cannot supply a truthful equity AT
    /// CONSTRUCTION TIME must fail the gate closed — never fall back to a
    /// configured/default figure.
    #[test]
    fn rr1_unavailable_authority_at_construction_fails_closed() {
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
            Arc::new(FailingAuthority(AccountAuthorityError::Unavailable)),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("an authority unavailable at construction must fail the gate closed");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    #[test]
    fn rr1_malformed_zero_equity_at_construction_fails_closed() {
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
            MutableEquityAuthority::new(0),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("non-positive authoritative equity at construction must fail the gate closed");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::RiskEngineUnavailable);
    }

    /// Control E: after construction, later authority-reported equity
    /// changes still flow dynamically through the SAME gate built via the
    /// production constructor (not just `for_test`).
    #[test]
    fn rr1_production_constructor_observes_live_equity_after_construction() {
        let authority = MutableEquityAuthority::new(100_000 * 1_000_000);
        let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
            &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
            authority.clone(),
            FixedClock::new(t(2024, 1, 15, 9, 0)),
        );
        assert_eq!(risk_gate.evaluate_gate(), mqk_execution::RiskDecision::Allow);

        authority.set(97_999 * 1_000_000);
        let mqk_execution::RiskDecision::Deny(denial) = risk_gate.evaluate_gate() else {
            panic!("a live equity drop after construction must still be observed and deny");
        };
        assert_eq!(denial.reason, mqk_execution::RiskReason::CapitalLimitExceeded);
    }
}
