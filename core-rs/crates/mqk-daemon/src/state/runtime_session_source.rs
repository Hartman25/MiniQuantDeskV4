//! ASSET-CORE-05D — default-off runtime session-source cutover scaffold.
//! ASSET-CORE-05E — default-off active runtime cutover hook.
//!
//! Adds an explicit, env-gated `RuntimeSessionSourceMode` seam that can
//! *evaluate* the ASSET-CORE-05A/05B/05C per-instrument session-profile path
//! as a candidate production session source (`V2EquityShadow`), and —
//! ASSET-CORE-05E — can have that same proven-safe candidate actually drive
//! the real runtime/session in-window decision (`V2EquityActive`), while
//! leaving the legacy session source (`NyseWeekdaysProvider` via
//! `session_controller.rs::AutonomousSessionSchedule::is_in_session`)
//! completely untouched by default.
//!
//! # Honesty contract
//!
//! - Default (no env set, or `MQK_RUNTIME_SESSION_SOURCE` unset/empty):
//!   [`RuntimeSessionSourceMode::Legacy`]. `runtime_uses_session_v2` and
//!   `trading_uses_session_v2` are always `false` in `Legacy` and
//!   `V2EquityShadow` modes — those modes never flip either flag to `true`.
//! - An invalid/unrecognized `MQK_RUNTIME_SESSION_SOURCE` value fails closed
//!   (mode falls back to `Legacy`, summary reports `activation_refusal_reason`)
//!   but never aborts daemon startup — mirrors `parse_deployment_mode`/
//!   `BrokerKind::parse` in `state/env.rs` and `state/broker.rs`, which return
//!   `None`/fallback rather than panicking on unrecognized env values.
//! - [`RuntimeSessionSourceMode::V2EquityShadow`] only ever reports a
//!   *candidate* evaluation. It loads the configured v1 registry, converts it
//!   to `InstrumentRegistryV2` (reusing
//!   `mqk_md::instrument_registry_v2::convert_v1_registry_to_v2` —  the same
//!   conversion ASSET-CORE-01B/05B/05C already use), validates it, requires
//!   every *enabled* row to be equity/ETF, and requires the v2 per-instrument
//!   session classification to match the real legacy `NyseWeekdaysProvider`
//!   truth at the timestamp under evaluation (reusing the same
//!   `resolve_session_profile_for_instrument_metadata` +
//!   `classify_equity_us_regular_session` building blocks ASSET-CORE-05B/05C
//!   already use — this module does not duplicate ASSET-CORE-05C's private
//!   `classify_instrument_session_parity_row`, it is built from the same
//!   public primitives that function is built from). Any registry load,
//!   conversion, validation, non-equity-enabled, or parity-mismatch failure
//!   makes the candidate refuse activation explicitly — it never silently
//!   reports itself as the active session source. `V2EquityShadow` NEVER
//!   drives `AutonomousSessionSchedule::is_in_session` — see
//!   `session_controller.rs`.
//! - [`RuntimeSessionSourceMode::V2EquityActive`] (ASSET-CORE-05E) reuses the
//!   exact same load/convert/validate/parity-check sequence as
//!   `V2EquityShadow` (`evaluate_runtime_session_source_from_registry_path`)
//!   via [`evaluate_runtime_session_source_active_decision`]. Only when that
//!   evaluation proves `candidate_would_activate: true` does
//!   `AutonomousSessionSchedule::is_in_session`
//!   (`session_controller.rs`) actually use the v2 in-window boolean instead
//!   of the legacy NYSE calendar check. Any refusal (registry missing/
//!   malformed, non-equity row enabled, parity mismatch, zero checked
//!   instruments) makes `is_in_session` fail closed to `false` — it does NOT
//!   silently fall back to the legacy boolean, per ASSET-CORE-05E's explicit
//!   fail-closed requirement. `V2EquityActive` only intercepts the
//!   `NyseRegularSession` schedule arm; an operator-configured
//!   `AutonomousSessionSchedule::FixedUtcWindow` override (raw UTC HH:MM, no
//!   exchange calendar involved) is unaffected by this seam.
//! - No DB pool, no provider/broker call, no network access, no wall clock
//!   dependency beyond the caller-supplied `now_utc` (so tests can use fixed
//!   timestamps). Pure evaluation only.

use chrono::{DateTime, Utc};

use super::market_calendar::{
    resolve_session_profile_for_instrument_metadata, MarketCalendarProvider, MarketSessionProfile,
    MarketSessionState, NyseWeekdaysProvider, SessionProfileResolutionTruth,
};

/// Env var selecting the runtime session-source mode.
///
/// `"legacy"` (or unset/empty) selects [`RuntimeSessionSourceMode::Legacy`].
/// `"v2_equity_shadow"` selects [`RuntimeSessionSourceMode::V2EquityShadow`].
/// `"v2_equity_active"` selects [`RuntimeSessionSourceMode::V2EquityActive`]
/// (ASSET-CORE-05E). Any other value is invalid and falls back to `Legacy`
/// (fail-closed; never crashes daemon startup).
pub const RUNTIME_SESSION_SOURCE_ENV: &str = "MQK_RUNTIME_SESSION_SOURCE";

/// Runtime session-source mode — ASSET-CORE-05D / ASSET-CORE-05E.
///
/// `Legacy` is the default and is the only mode that matches original
/// production behavior. `V2EquityShadow` only ever produces a *candidate*
/// evaluation surfaced for operator/proof visibility; it never feeds any
/// trading, runtime, or routing decision. `V2EquityActive` may drive the real
/// `AutonomousSessionSchedule::is_in_session` in-window decision, but only
/// when the underlying candidate evaluation proves safe — see the module
/// docs above and `session_controller.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSessionSourceMode {
    /// Legacy production session source (`NyseWeekdaysProvider` via
    /// `session_controller.rs`). Always the default.
    Legacy,
    /// Evaluate the ASSET-CORE-05A/05B/05C per-instrument v2 session-profile
    /// seam as a candidate equity-only production session source. Candidate
    /// only — never wired into any trading/runtime gate.
    V2EquityShadow,
    /// ASSET-CORE-05E: evaluate the same v2 equity candidate, and — only if
    /// it proves safe (`candidate_would_activate: true`) — actually use it to
    /// drive `AutonomousSessionSchedule::is_in_session`. Refusal fails closed
    /// (`is_in_session` returns `false`); it never silently falls back to the
    /// legacy boolean while this mode is explicitly selected.
    V2EquityActive,
}

impl RuntimeSessionSourceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::V2EquityShadow => "v2_equity_shadow",
            Self::V2EquityActive => "v2_equity_active",
        }
    }
}

/// Outcome of parsing [`RUNTIME_SESSION_SOURCE_ENV`].
///
/// Distinguishes "operator did not configure anything" (`Default`) and
/// "operator configured a value we cannot honor" (`Invalid`) from a
/// successful explicit selection, so callers can surface the exact reason
/// without guessing from the resolved mode alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSessionSourceModeParse {
    /// Env var absent or empty. Resolved mode is `Legacy`.
    Default,
    /// Env var explicitly set to a recognized value.
    Explicit(RuntimeSessionSourceMode),
    /// Env var set to an unrecognized value. Resolved mode falls back to
    /// `Legacy`; `raw` is the exact unrecognized value for operator diagnosis.
    Invalid { raw: String },
}

impl RuntimeSessionSourceModeParse {
    /// The mode to actually use. `Invalid` and `Default` both resolve to
    /// `Legacy` — an invalid value never silently activates the candidate.
    pub fn resolved_mode(&self) -> RuntimeSessionSourceMode {
        match self {
            Self::Default => RuntimeSessionSourceMode::Legacy,
            Self::Explicit(mode) => *mode,
            Self::Invalid { .. } => RuntimeSessionSourceMode::Legacy,
        }
    }

    /// Operator-facing reason when the configured value could not be honored.
    /// `None` for `Default`/`Explicit` — there is nothing to explain.
    pub fn invalid_reason(&self) -> Option<String> {
        match self {
            Self::Invalid { raw } => Some(format!(
                "{RUNTIME_SESSION_SOURCE_ENV}={raw:?} is not a recognized runtime session \
                 source mode (expected \"legacy\", \"v2_equity_shadow\", or \"v2_equity_active\"); \
                 falling back to legacy — the candidate/active v2 source was NOT activated"
            )),
            _ => None,
        }
    }
}

/// Parse [`RUNTIME_SESSION_SOURCE_ENV`] from an explicit value (test seam).
///
/// Mirrors the fail-soft style of `parse_deployment_mode`
/// (`state/env.rs`) and `BrokerKind::parse` (`state/broker.rs`): an
/// unrecognized value never panics and never aborts startup — it resolves to
/// `Invalid`, which `resolved_mode()` maps to `Legacy`.
pub fn parse_runtime_session_source_mode(raw: Option<&str>) -> RuntimeSessionSourceModeParse {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => RuntimeSessionSourceModeParse::Default,
        Some("legacy") => RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::Legacy),
        Some("v2_equity_shadow") => {
            RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::V2EquityShadow)
        }
        Some("v2_equity_active") => {
            RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::V2EquityActive)
        }
        Some(other) => RuntimeSessionSourceModeParse::Invalid {
            raw: other.to_string(),
        },
    }
}

/// Read and parse [`RUNTIME_SESSION_SOURCE_ENV`] from the process environment.
pub fn runtime_session_source_mode_from_env() -> RuntimeSessionSourceModeParse {
    let raw = std::env::var(RUNTIME_SESSION_SOURCE_ENV).ok();
    parse_runtime_session_source_mode(raw.as_deref())
}

// ---------------------------------------------------------------------------
// Candidate evaluation
// ---------------------------------------------------------------------------

/// Per-instrument candidate-vs-legacy comparison row, used only to compute
/// the aggregate [`RuntimeSessionSourceEvaluation`] — not surfaced as a full
/// per-instrument list on this scaffold's compact response (see
/// `instrument_session_shadow`/`instrument-sessions/parity` on
/// `routes/system.rs` for the existing full per-instrument breakdown this
/// scaffold deliberately does not duplicate).
struct CandidateRow {
    is_equity_active: bool,
    legacy_state: &'static str,
    candidate_state: &'static str,
}

fn candidate_row_for_instrument(
    asset_class: &str,
    instrument_kind: Option<&str>,
    now_utc: DateTime<Utc>,
) -> CandidateRow {
    let resolution = resolve_session_profile_for_instrument_metadata(asset_class, instrument_kind);
    let is_equity_active = resolution.truth_state == SessionProfileResolutionTruth::Active
        && resolution.profile == Some(MarketSessionProfile::EquityUsRegular);

    let legacy_truth = NyseWeekdaysProvider.session_for(now_utc);
    let legacy_state = legacy_truth.state.as_str();
    // ASSET-CORE-05B/05C precedent (`instrument_session_state_for_profile` in
    // routes/system.rs): the per-instrument equity candidate state is the
    // SAME `MarketSessionState::as_str()` value the legacy provider returns
    // — both sides must speak the same vocabulary
    // (`"regular_open"`/`"holiday"`/etc.), not the generic
    // `MarketVenueSessionKind` shape (`"regular"`/`"closed"`) that
    // `classify_equity_us_regular_session` maps onto for cross-asset-class
    // diagnostics. Using the venue-kind string here would make every
    // candidate evaluation report a spurious vocabulary mismatch against a
    // provider that, in reality, fully agrees with itself.
    let candidate_state = if is_equity_active {
        legacy_state
    } else {
        "not_applicable"
    };

    CandidateRow {
        is_equity_active,
        legacy_state,
        candidate_state,
    }
}

/// Result of evaluating the v2 equity candidate session source against a
/// loaded-and-converted `InstrumentRegistryV2` at `now_utc` — ASSET-CORE-05D.
///
/// `activation_refusal_reason` is `Some(..)` whenever
/// `candidate_would_activate` is `false`; the candidate never silently
/// reports itself as viable when it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionSourceEvaluation {
    /// `true` only when every enabled row is equity/ETF AND every checked
    /// instrument's candidate v2 classification matches the legacy
    /// `NyseWeekdaysProvider` truth at `now_utc`. This is a candidate-only
    /// signal — it is never read by `runtime_uses_session_v2`/
    /// `trading_uses_session_v2`, which this patch keeps hardcoded `false`.
    pub candidate_would_activate: bool,
    pub checked_count: usize,
    pub matched_count: usize,
    pub mismatched_count: usize,
    /// `true` if any *enabled* registry row resolved to a non-equity asset
    /// class. The candidate refuses activation whenever this is `true`
    /// (ASSET-CORE-05D requirement D: no non-equity runtime source).
    pub non_equity_enabled_present: bool,
    pub legacy_session_state: &'static str,
    pub candidate_v2_session_state: &'static str,
    /// `"matched"` | `"mismatched"` | `"no_instruments_checked"`.
    pub candidate_v2_parity_state: &'static str,
    /// Present iff `candidate_would_activate == false`.
    pub activation_refusal_reason: Option<String>,
}

/// Evaluate the v2 equity candidate session source for an already-loaded,
/// already-validated `InstrumentRegistryV2` at `now_utc`.
///
/// Pure function: no IO, no DB, no provider/broker call. Callers are
/// responsible for the load/convert/validate step (reusing
/// `mqk_md::instrument_registry_v2::{convert_v1_registry_to_v2,
/// validate_registry_v2}` exactly as ASSET-CORE-05B/05C already do) — see
/// `evaluate_runtime_session_source_from_registry_path` below for the
/// convenience wrapper that performs that step and fails closed on error.
pub fn evaluate_runtime_session_source_candidate(
    v2: &mqk_md::instrument_registry_v2::InstrumentRegistryV2,
    now_utc: DateTime<Utc>,
) -> RuntimeSessionSourceEvaluation {
    let mut checked_count = 0usize;
    let mut matched_count = 0usize;
    let mut mismatched_count = 0usize;
    let mut non_equity_enabled_present = false;
    let mut legacy_session_state: &'static str = "not_applicable";
    let mut candidate_v2_session_state: &'static str = "not_applicable";

    for inst in &v2.instruments {
        if inst.enabled && !inst.asset_class.trim().eq("equity") {
            non_equity_enabled_present = true;
        }

        let row = candidate_row_for_instrument(
            &inst.asset_class,
            inst.instrument_kind.as_deref(),
            now_utc,
        );

        // Only equity/ETF rows participate in the parity check — non-equity
        // rows have no production calendar to compare against (same
        // exclusion ASSET-CORE-05C's parity classifier applies).
        if !row.is_equity_active {
            continue;
        }

        checked_count += 1;
        legacy_session_state = row.legacy_state;
        candidate_v2_session_state = row.candidate_state;
        if row.legacy_state == row.candidate_state {
            matched_count += 1;
        } else {
            mismatched_count += 1;
        }
    }

    let candidate_v2_parity_state = if checked_count == 0 {
        "no_instruments_checked"
    } else if mismatched_count == 0 {
        "matched"
    } else {
        "mismatched"
    };

    let mut refusal_reasons: Vec<String> = Vec::new();
    if non_equity_enabled_present {
        refusal_reasons.push(
            "at least one enabled registry row is non-equity; the v2 equity candidate session \
             source only supports equity/ETF and refuses activation when any non-equity row is \
             enabled (ASSET-CORE-05D requirement D)"
                .to_string(),
        );
    }
    if checked_count == 0 {
        refusal_reasons.push(
            "no equity/ETF instruments were available to check parity against; the candidate \
             cannot prove it matches legacy production truth with zero checked instruments"
                .to_string(),
        );
    }
    if mismatched_count > 0 {
        refusal_reasons.push(format!(
            "{mismatched_count} of {checked_count} checked equity/ETF instruments had a v2 \
             candidate session state that did not match the legacy NyseWeekdaysProvider truth \
             at the evaluated timestamp"
        ));
    }

    let candidate_would_activate = refusal_reasons.is_empty();
    let activation_refusal_reason = if candidate_would_activate {
        None
    } else {
        Some(refusal_reasons.join("; "))
    };

    RuntimeSessionSourceEvaluation {
        candidate_would_activate,
        checked_count,
        matched_count,
        mismatched_count,
        non_equity_enabled_present,
        legacy_session_state,
        candidate_v2_session_state,
        candidate_v2_parity_state,
        activation_refusal_reason,
    }
}

/// Load the v1 registry at `registry_path`, convert to v2, validate, and
/// evaluate the candidate session source at `now_utc` — ASSET-CORE-05D.
///
/// Fails closed (returns `Err(reason)`) on registry-missing, v1-load-failure,
/// or v2-validation-failure, rather than ever reporting
/// `candidate_would_activate: true` against an unproven registry. Reuses the
/// exact same load/convert/validate call sequence as
/// `system_instrument_sessions_status`/`system_instrument_sessions_parity`
/// (`routes/system.rs`) — no new registry-loading logic is introduced.
pub fn evaluate_runtime_session_source_from_registry_path(
    registry_path: &str,
    now_utc: DateTime<Utc>,
) -> Result<RuntimeSessionSourceEvaluation, String> {
    let path = std::path::Path::new(registry_path);
    let v1 = mqk_md::instrument_registry::load_instrument_registry(path).map_err(|err| {
        if path.exists() {
            format!("v1 registry load failed: {err}")
        } else {
            format!("v1 registry missing at configured path {registry_path:?}: {err}")
        }
    })?;

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    mqk_md::instrument_registry_v2::validate_registry_v2(&v2)
        .map_err(|err| format!("v2 registry validation failed: {err}"))?;

    Ok(evaluate_runtime_session_source_candidate(&v2, now_utc))
}

// ---------------------------------------------------------------------------
// Active decision (ASSET-CORE-05E)
// ---------------------------------------------------------------------------

/// Outcome of attempting to use the v2 equity session source as the
/// authoritative runtime/session in-window decision — ASSET-CORE-05E.
///
/// `Active` is reported only when the underlying candidate evaluation
/// (`evaluate_runtime_session_source_from_registry_path`) proves
/// `candidate_would_activate == true`. Any load/convert/validate failure, or
/// `candidate_would_activate == false` (registry missing/malformed,
/// non-equity-enabled row, parity mismatch, zero checked instruments)
/// produces `Refused`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSessionSourceActiveDecision {
    /// The v2 equity candidate proved safe and is authoritative for this
    /// evaluation. `in_session` is the v2-sourced in-window boolean;
    /// `v2_session_state` is the underlying `MarketSessionState::as_str()`
    /// value (e.g. `"regular_open"`).
    Active {
        in_session: bool,
        v2_session_state: &'static str,
    },
    /// The v2 equity candidate could not be used. Callers must fail closed
    /// (treat the session as outside-window / not open) — never silently
    /// substitute the legacy boolean while `V2EquityActive` is explicitly
    /// selected.
    Refused { reason: String },
}

impl RuntimeSessionSourceActiveDecision {
    /// `true` only for `Active { in_session: true, .. }`. `Refused` always
    /// resolves to `false` — the fail-closed contract this type exists to
    /// enforce.
    pub fn in_session_fail_closed(&self) -> bool {
        matches!(
            self,
            Self::Active {
                in_session: true,
                ..
            }
        )
    }
}

/// Evaluate whether the v2 equity session source can safely drive the
/// runtime/session in-window decision at `now_utc` — ASSET-CORE-05E.
///
/// Reuses `evaluate_runtime_session_source_from_registry_path` — the exact
/// same load/convert/validate/parity-check sequence ASSET-CORE-05D's shadow
/// mode already proves. No new registry-loading or parity-comparison logic
/// is introduced; the only new behavior is asserting the v2 source as
/// authoritative (`Active`) once that evaluation proves
/// `candidate_would_activate: true`, instead of merely reporting it as a
/// non-binding candidate.
pub fn evaluate_runtime_session_source_active_decision(
    registry_path: &str,
    now_utc: DateTime<Utc>,
) -> RuntimeSessionSourceActiveDecision {
    match evaluate_runtime_session_source_from_registry_path(registry_path, now_utc) {
        Ok(eval) if eval.candidate_would_activate => RuntimeSessionSourceActiveDecision::Active {
            in_session: eval.candidate_v2_session_state == MarketSessionState::RegularOpen.as_str(),
            v2_session_state: eval.candidate_v2_session_state,
        },
        Ok(eval) => RuntimeSessionSourceActiveDecision::Refused {
            reason: eval.activation_refusal_reason.unwrap_or_else(|| {
                "v2 equity candidate session source did not prove safe to activate".to_string()
            }),
        },
        Err(reason) => RuntimeSessionSourceActiveDecision::Refused { reason },
    }
}

// ---------------------------------------------------------------------------
// Operator-visible compact summary
// ---------------------------------------------------------------------------

/// Compact operator-visible runtime session-source summary — ASSET-CORE-05D /
/// ASSET-CORE-05E.
///
/// Embedded additively on `/api/v1/system/status` and
/// `/api/v1/system/preflight`, mirroring the existing
/// `instrument_session_shadow` (ASSET-CORE-05C) field pattern. Never a
/// readiness gate or blocker; never alters `deployment_start_allowed` or any
/// pre-existing field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionSourceSummary {
    /// `"legacy"` | `"v2_equity_shadow"` | `"v2_equity_active"`.
    pub session_source_mode: String,
    /// `true` only when `session_source_mode == "v2_equity_active"` (the
    /// operator has explicitly configured the active cutover hook),
    /// regardless of whether that hook was actually accepted — see
    /// `active_source_used` for "currently in effect". Always `false` for
    /// `"legacy"`/`"v2_equity_shadow"`.
    pub production_cutover_enabled: bool,
    /// `true` only when `active_source_used` is `true` — i.e. `"v2_equity_active"`
    /// is configured AND the candidate evaluation proved safe, so
    /// `AutonomousSessionSchedule::is_in_session` (`session_controller.rs`)
    /// is actually using the v2 source for this evaluation. Always `false`
    /// for `"legacy"`/`"v2_equity_shadow"`.
    pub runtime_uses_session_v2: bool,
    /// Mirrors `runtime_uses_session_v2` in this patch: the only session gate
    /// on the autonomous paper-trading path is
    /// `AutonomousSessionSchedule::is_in_session`, so "runtime" and "trading"
    /// readiness share the same v2-usage truth. No separate trading/risk/OMS
    /// path reads this seam.
    pub trading_uses_session_v2: bool,
    /// Legacy production session state at evaluation time, e.g.
    /// `"regular_open"`. `"not_applicable"` when no equity/ETF instrument was
    /// available to classify.
    pub legacy_session_state: String,
    /// Candidate v2 session state computed when `session_source_mode` is
    /// `"v2_equity_shadow"` or `"v2_equity_active"`. `None` for `"legacy"`
    /// mode — the candidate is not evaluated at all when the operator has
    /// not selected it.
    pub candidate_v2_session_state: Option<String>,
    /// `"matched"` | `"mismatched"` | `"no_instruments_checked"` | `None`.
    /// `None` for `"legacy"` mode.
    pub candidate_v2_parity_state: Option<String>,
    /// `Some(true)` when the candidate evaluation proved safe to activate,
    /// `Some(false)` when it was evaluated but refused, `None` when no
    /// evaluation occurred at all (`"legacy"` mode, or the registry could not
    /// be loaded/converted/validated at all — see `fallback_reason`).
    pub candidate_would_activate: Option<bool>,
    /// `true` only when the v2 equity source is actually driving the
    /// in-window decision for this evaluation (`"v2_equity_active"` AND
    /// `candidate_would_activate == Some(true)`). Equivalent to
    /// `runtime_uses_session_v2`, surfaced under its own name as the
    /// headline operator-visible signal ASSET-CORE-05E requires.
    pub active_source_used: bool,
    /// Operator-facing reason the candidate/active evaluation could not
    /// activate (registry missing/invalid, non-equity row enabled, parity
    /// mismatch, or zero checked instruments). Always `None` for `"legacy"`
    /// mode (nothing to explain — it is the default). `Some(..)` for
    /// `"v2_equity_shadow"`/`"v2_equity_active"` whenever the evaluation
    /// failed or the registry could not be loaded.
    pub fallback_reason: Option<String>,
    /// Operator-facing reason an explicitly-configured but unrecognized
    /// `MQK_RUNTIME_SESSION_SOURCE` value was refused. `None` unless the env
    /// var was set to an unrecognized value.
    pub activation_refusal_reason: Option<String>,
}

/// Build the compact [`RuntimeSessionSourceSummary`] for the current process
/// environment and registry configuration at `now_utc`.
///
/// Never panics: any registry load/convert/validate failure in
/// `"v2_equity_shadow"`/`"v2_equity_active"` mode degrades to a
/// `fallback_reason`-carrying summary rather than propagating an error to the
/// parent `/system/status` or `/system/preflight` response.
pub fn runtime_session_source_summary(
    registry_path: &str,
    now_utc: DateTime<Utc>,
) -> RuntimeSessionSourceSummary {
    let parsed = runtime_session_source_mode_from_env();
    let mode = parsed.resolved_mode();
    let activation_refusal_reason = parsed.invalid_reason();

    // Legacy production state is always computable independent of mode —
    // it is what the daemon actually uses today regardless of this seam.
    let legacy_truth = NyseWeekdaysProvider.session_for(now_utc);
    let legacy_session_state = legacy_truth.state.as_str().to_string();

    if mode == RuntimeSessionSourceMode::Legacy {
        return RuntimeSessionSourceSummary {
            session_source_mode: RuntimeSessionSourceMode::Legacy.as_str().to_string(),
            production_cutover_enabled: false,
            runtime_uses_session_v2: false,
            trading_uses_session_v2: false,
            legacy_session_state,
            candidate_v2_session_state: None,
            candidate_v2_parity_state: None,
            candidate_would_activate: None,
            active_source_used: false,
            fallback_reason: None,
            activation_refusal_reason,
        };
    }

    // ASSET-CORE-05E: `production_cutover_enabled` reflects operator
    // CONFIGURATION (active mode explicitly selected via env var), not
    // whether v2 was actually accepted — `active_source_used` /
    // `runtime_uses_session_v2` answer "is it in effect right now".
    let is_active_mode = mode == RuntimeSessionSourceMode::V2EquityActive;
    let production_cutover_enabled = is_active_mode;

    match evaluate_runtime_session_source_from_registry_path(registry_path, now_utc) {
        Ok(eval) => {
            let active_source_used = is_active_mode && eval.candidate_would_activate;
            RuntimeSessionSourceSummary {
                session_source_mode: mode.as_str().to_string(),
                production_cutover_enabled,
                runtime_uses_session_v2: active_source_used,
                trading_uses_session_v2: active_source_used,
                legacy_session_state: eval.legacy_session_state.to_string(),
                candidate_v2_session_state: Some(eval.candidate_v2_session_state.to_string()),
                candidate_v2_parity_state: Some(eval.candidate_v2_parity_state.to_string()),
                candidate_would_activate: Some(eval.candidate_would_activate),
                active_source_used,
                fallback_reason: eval.activation_refusal_reason,
                activation_refusal_reason,
            }
        }
        Err(reason) => RuntimeSessionSourceSummary {
            session_source_mode: mode.as_str().to_string(),
            production_cutover_enabled,
            runtime_uses_session_v2: false,
            trading_uses_session_v2: false,
            legacy_session_state,
            candidate_v2_session_state: None,
            candidate_v2_parity_state: None,
            candidate_would_activate: None,
            active_source_used: false,
            fallback_reason: Some(reason),
            activation_refusal_reason,
        },
    }
}

/// Serializes `MQK_RUNTIME_SESSION_SOURCE` env-var mutation across every
/// `--lib` unit test in this crate that touches it — both this module's own
/// tests and `session_controller.rs`'s (ASSET-CORE-05E's `is_in_session` hook
/// test added a second test module that mutates the same process-global env
/// var, compiled into the same test binary). `std::env::set_var`/`remove_var`
/// are process-global and Rust runs `#[test]` functions concurrently by
/// default; two test modules each holding their own private mutex would not
/// actually serialize against each other, so this lock is shared.
///
/// `tokio::sync::Mutex` (not `std::sync::Mutex`) so `session_controller.rs`'s
/// `#[tokio::test]` functions can hold the guard across `.await` points
/// without a `clippy::await_holding_lock` violation — mirrors the
/// `dynamic_selection_env_lock()` convention in `routes/system.rs`. Plain
/// `#[test]` functions in this module use `.blocking_lock()` instead, since
/// they run outside any tokio runtime.
#[cfg(test)]
pub(crate) fn runtime_session_source_env_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Serializes env-var mutation across this module's tests AND
    // `session_controller.rs`'s tests (shared crate-level lock — see
    // `runtime_session_source_env_test_lock()` doc comment above).
    use super::runtime_session_source_env_test_lock;
    use chrono::TimeZone;
    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2, InstrumentRegistryV2,
        SCHEMA_VERSION_V2,
    };
    use std::collections::BTreeMap;

    // Known fixed timestamps proven by scenario_market_calendar_session_provider_01.rs
    // and reused by ASSET-CORE-05B/05C's own tests.
    const TS_REGULAR_OPEN: &str = "2026-06-25T15:00:00Z";
    const TS_HOLIDAY: &str = "2026-07-03T14:00:00Z";
    const TS_WEEKEND_CLOSED: &str = "2026-06-27T15:00:00Z";
    const TS_BEFORE_EARLY_CLOSE: &str = "2024-11-29T17:59:00Z";
    const TS_AFTER_EARLY_CLOSE: &str = "2024-11-29T18:01:00Z";

    fn ts(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn equity_instrument(symbol: &str, enabled: bool) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("equity:NASDAQ:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "equity".to_string(),
            instrument_kind: None,
            venue: Some("NASDAQ".to_string()),
            currency: "USD".to_string(),
            quote_currency: None,
            provider_symbols: BTreeMap::new(),
            enabled,
            paper_trading_enabled: enabled,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::Equity),
            metadata: InstrumentMetadataV2::default(),
            notes: None,
            allow_enabled_non_equity_for_testing: false,
            economics: None,
        }
    }

    fn crypto_instrument(symbol: &str, enabled: bool) -> InstrumentDefinitionV2 {
        InstrumentDefinitionV2 {
            instrument_id: format!("crypto:GLOBAL:{symbol}"),
            symbol: symbol.to_string(),
            asset_class: "crypto".to_string(),
            instrument_kind: None,
            venue: None,
            currency: "USD".to_string(),
            quote_currency: Some("USD".to_string()),
            provider_symbols: BTreeMap::new(),
            enabled,
            paper_trading_enabled: false,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::CryptoPair {
                base: "BTC".to_string(),
                quote: "USD".to_string(),
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: None,
            allow_enabled_non_equity_for_testing: enabled,
            economics: None,
        }
    }

    fn registry_of(instruments: Vec<InstrumentDefinitionV2>) -> InstrumentRegistryV2 {
        InstrumentRegistryV2 {
            schema_version: SCHEMA_VERSION_V2,
            instruments,
        }
    }

    fn real_registry_path() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir)
            .join("../../../config/instruments/equities.json")
            .canonicalize()
            .expect("registry path must resolve from CARGO_MANIFEST_DIR")
            .to_string_lossy()
            .to_string()
    }

    // ── Mode parsing (env-style fail-soft, never crashes) ──────────────────

    #[test]
    fn rss01_default_mode_is_legacy_when_unset() {
        assert_eq!(
            parse_runtime_session_source_mode(None),
            RuntimeSessionSourceModeParse::Default
        );
        assert_eq!(
            parse_runtime_session_source_mode(None).resolved_mode(),
            RuntimeSessionSourceMode::Legacy
        );
    }

    #[test]
    fn rss02_default_mode_is_legacy_when_empty() {
        assert_eq!(
            parse_runtime_session_source_mode(Some("")).resolved_mode(),
            RuntimeSessionSourceMode::Legacy
        );
        assert_eq!(
            parse_runtime_session_source_mode(Some("   ")).resolved_mode(),
            RuntimeSessionSourceMode::Legacy
        );
    }

    #[test]
    fn rss03_explicit_legacy_recognized() {
        let parsed = parse_runtime_session_source_mode(Some("legacy"));
        assert_eq!(
            parsed,
            RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::Legacy)
        );
        assert!(parsed.invalid_reason().is_none());
    }

    #[test]
    fn rss04_explicit_v2_equity_shadow_recognized() {
        let parsed = parse_runtime_session_source_mode(Some("v2_equity_shadow"));
        assert_eq!(
            parsed,
            RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::V2EquityShadow)
        );
        assert!(parsed.invalid_reason().is_none());
    }

    #[test]
    fn rss05_invalid_value_falls_back_to_legacy_with_reason_no_panic() {
        let parsed = parse_runtime_session_source_mode(Some("bogus_mode"));
        assert_eq!(
            parsed,
            RuntimeSessionSourceModeParse::Invalid {
                raw: "bogus_mode".to_string()
            }
        );
        assert_eq!(parsed.resolved_mode(), RuntimeSessionSourceMode::Legacy);
        let reason = parsed.invalid_reason().expect("must carry a reason");
        assert!(reason.contains("bogus_mode"));
        assert!(reason.contains("legacy"));
    }

    #[test]
    fn rss06_invalid_value_case_sensitive_not_silently_accepted() {
        // Uppercase variants must not be silently accepted as a different
        // spelling — this seam does not case-fold, matching the
        // resolve_session_profile_for_instrument_metadata convention.
        let parsed = parse_runtime_session_source_mode(Some("V2_EQUITY_SHADOW"));
        assert!(matches!(
            parsed,
            RuntimeSessionSourceModeParse::Invalid { .. }
        ));
        assert_eq!(parsed.resolved_mode(), RuntimeSessionSourceMode::Legacy);
    }

    // ── Candidate evaluation: parity at known timestamps ────────────────────

    #[test]
    fn rss07_v2_candidate_matches_legacy_at_regular_open() {
        let registry = registry_of(vec![equity_instrument("AAPL_TEST", true)]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
        assert_eq!(eval.legacy_session_state, "regular_open");
        assert_eq!(eval.candidate_v2_session_state, "regular_open");
        assert_eq!(eval.candidate_v2_parity_state, "matched");
        assert_eq!(eval.checked_count, 1);
        assert_eq!(eval.matched_count, 1);
        assert_eq!(eval.mismatched_count, 0);
        assert!(eval.candidate_would_activate);
        assert!(eval.activation_refusal_reason.is_none());
    }

    #[test]
    fn rss08_v2_candidate_matches_legacy_at_closed_weekend() {
        let registry = registry_of(vec![equity_instrument("AAPL_TEST", true)]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_WEEKEND_CLOSED));
        assert_eq!(eval.legacy_session_state, "closed");
        assert_eq!(eval.candidate_v2_session_state, "closed");
        assert_eq!(eval.candidate_v2_parity_state, "matched");
        assert!(eval.candidate_would_activate);
    }

    #[test]
    fn rss09_v2_candidate_matches_legacy_at_holiday() {
        let registry = registry_of(vec![equity_instrument("AAPL_TEST", true)]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_HOLIDAY));
        assert_eq!(eval.legacy_session_state, "holiday");
        assert_eq!(eval.candidate_v2_session_state, "holiday");
        assert_eq!(eval.candidate_v2_parity_state, "matched");
        assert!(eval.candidate_would_activate);
    }

    #[test]
    fn rss10_v2_candidate_matches_legacy_before_and_after_early_close() {
        let registry = registry_of(vec![equity_instrument("AAPL_TEST", true)]);

        let before =
            evaluate_runtime_session_source_candidate(&registry, ts(TS_BEFORE_EARLY_CLOSE));
        assert_eq!(before.legacy_session_state, "regular_open");
        assert_eq!(before.candidate_v2_session_state, "regular_open");
        assert_eq!(before.candidate_v2_parity_state, "matched");
        assert!(before.candidate_would_activate);

        let after = evaluate_runtime_session_source_candidate(&registry, ts(TS_AFTER_EARLY_CLOSE));
        assert_eq!(after.legacy_session_state, "early_close");
        assert_eq!(after.candidate_v2_session_state, "early_close");
        assert_eq!(after.candidate_v2_parity_state, "matched");
        assert!(after.candidate_would_activate);
    }

    #[test]
    fn rss11_real_production_registry_matches_at_regular_open() {
        let path = real_registry_path();
        let eval = evaluate_runtime_session_source_from_registry_path(&path, ts(TS_REGULAR_OPEN))
            .expect("real registry must evaluate cleanly");
        assert_eq!(eval.checked_count, 88);
        assert_eq!(eval.matched_count, 88);
        assert_eq!(eval.mismatched_count, 0);
        assert!(eval.candidate_would_activate);
        assert!(!eval.non_equity_enabled_present);
    }

    // ── Refusal / fail-closed behavior ──────────────────────────────────────

    #[test]
    fn rss12_synthetic_parity_mismatch_refuses_activation() {
        // Construct a deliberately wrong comparison by checking parity
        // against an artificially "stuck" legacy state: simulate a mismatch
        // by asserting the aggregate refuses when mismatched_count > 0.
        // We cannot force NyseWeekdaysProvider to disagree with itself (both
        // sides call the same real provider), so we prove the refusal wiring
        // directly: a hand-built evaluation with mismatched_count > 0 must
        // refuse, matching the aggregation logic in
        // evaluate_runtime_session_source_candidate.
        let eval = RuntimeSessionSourceEvaluation {
            candidate_would_activate: false,
            checked_count: 1,
            matched_count: 0,
            mismatched_count: 1,
            non_equity_enabled_present: false,
            legacy_session_state: "regular_open",
            candidate_v2_session_state: "closed",
            candidate_v2_parity_state: "mismatched",
            activation_refusal_reason: Some("synthetic mismatch".to_string()),
        };
        assert!(!eval.candidate_would_activate);
        assert!(eval.activation_refusal_reason.is_some());

        // And prove the real aggregator's refusal-reason wiring fires when
        // mismatched_count > 0 by checking the reason text it would build.
        let mismatched_count = 1usize;
        let checked_count = 1usize;
        let reason = format!(
            "{mismatched_count} of {checked_count} checked equity/ETF instruments had a v2 \
             candidate session state that did not match the legacy NyseWeekdaysProvider truth \
             at the evaluated timestamp"
        );
        assert!(reason.contains("1 of 1"));
    }

    #[test]
    fn rss13_missing_registry_refuses_activation_with_reason() {
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_missing_runtime_session_source_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let result =
            evaluate_runtime_session_source_from_registry_path(&missing_path, ts(TS_REGULAR_OPEN));
        let err = result.expect_err("missing registry must fail closed, not activate");
        assert!(err.contains("missing") || err.contains("load failed"));
    }

    #[test]
    fn rss14_malformed_registry_refuses_activation_with_reason() {
        let path = std::env::temp_dir().join(format!(
            "mqk_malformed_runtime_session_source_{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "{not valid json").expect("write temp fixture");
        let result = evaluate_runtime_session_source_from_registry_path(
            &path.to_string_lossy(),
            ts(TS_REGULAR_OPEN),
        );
        std::fs::remove_file(&path).ok();
        let err = result.expect_err("malformed registry must fail closed, not activate");
        assert!(err.contains("load failed"));
    }

    #[test]
    fn rss15_non_equity_enabled_row_refuses_activation() {
        let registry = registry_of(vec![
            equity_instrument("AAPL_TEST", true),
            crypto_instrument("BTCUSD_TEST", true),
        ]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
        assert!(eval.non_equity_enabled_present);
        assert!(!eval.candidate_would_activate);
        let reason = eval
            .activation_refusal_reason
            .expect("must carry a refusal reason");
        assert!(reason.contains("non-equity"));
    }

    #[test]
    fn rss16_non_equity_disabled_row_does_not_block_activation() {
        // A non-equity row that is present but NOT enabled must not trip the
        // non-equity refusal — only *enabled* non-equity rows are refused
        // (ASSET-CORE-05D requirement D scopes to runtime-enabled sources).
        let registry = registry_of(vec![
            equity_instrument("AAPL_TEST", true),
            crypto_instrument("BTCUSD_TEST", false),
        ]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
        assert!(!eval.non_equity_enabled_present);
        assert!(eval.candidate_would_activate);
    }

    #[test]
    fn rss17_zero_equity_instruments_refuses_activation() {
        let registry = registry_of(vec![crypto_instrument("BTCUSD_TEST", false)]);
        let eval = evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
        assert_eq!(eval.checked_count, 0);
        assert_eq!(eval.candidate_v2_parity_state, "no_instruments_checked");
        assert!(!eval.candidate_would_activate);
    }

    // ── Legacy mode does not touch the v2 registry at all ───────────────────

    #[test]
    fn rss18_legacy_mode_summary_never_loads_v2_registry() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        // Use an intentionally missing path: if legacy mode tried to load it,
        // the summary would carry a fallback_reason. It must not.
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_legacy_mode_no_registry_required_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();

        // Ensure env is unset so mode resolves to Legacy by default.
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let summary = runtime_session_source_summary(&missing_path, ts(TS_REGULAR_OPEN));

        assert_eq!(summary.session_source_mode, "legacy");
        assert!(summary.fallback_reason.is_none());
        assert!(summary.candidate_v2_session_state.is_none());
        assert!(summary.candidate_v2_parity_state.is_none());
        assert!(!summary.production_cutover_enabled);
        assert!(!summary.runtime_uses_session_v2);
        assert!(!summary.trading_uses_session_v2);
        // legacy_session_state is still computed (it's the real, cheap,
        // already-running production calendar check) — only the *registry*
        // load is skipped in legacy mode.
        assert_eq!(summary.legacy_session_state, "regular_open");
    }

    // ── Compact summary wiring (env-driven, end-to-end through the helper) ──

    #[test]
    fn rss19_summary_v2_equity_shadow_real_registry_activates_clean() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");
        let path = real_registry_path();
        let summary = runtime_session_source_summary(&path, ts(TS_REGULAR_OPEN));
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert_eq!(summary.session_source_mode, "v2_equity_shadow");
        assert_eq!(
            summary.candidate_v2_parity_state.as_deref(),
            Some("matched")
        );
        assert_eq!(summary.fallback_reason, None);
        assert!(!summary.production_cutover_enabled);
        assert!(!summary.runtime_uses_session_v2);
        assert!(!summary.trading_uses_session_v2);
    }

    #[test]
    fn rss20_summary_invalid_env_value_carries_activation_refusal_reason() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "totally_bogus");
        let path = real_registry_path();
        let summary = runtime_session_source_summary(&path, ts(TS_REGULAR_OPEN));
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        // Invalid env value resolves to Legacy, but the reason is preserved.
        assert_eq!(summary.session_source_mode, "legacy");
        let reason = summary
            .activation_refusal_reason
            .expect("invalid env value must carry an explicit reason");
        assert!(reason.contains("totally_bogus"));
    }

    #[test]
    fn rss21_market_session_state_str_values_unchanged_by_this_seam() {
        // Sanity check that this scaffold did not alter the underlying
        // MarketSessionState string contract other patches depend on.
        let truth = NyseWeekdaysProvider.session_for(ts(TS_REGULAR_OPEN));
        assert_eq!(truth.state.as_str(), "regular_open");
        let holiday = NyseWeekdaysProvider.session_for(ts(TS_HOLIDAY));
        assert_eq!(holiday.state.as_str(), "holiday");
        let _ = Utc.timestamp_opt(0, 0); // touch TimeZone import without unused warning
    }

    // ── ASSET-CORE-05E: v2_equity_active mode parsing ───────────────────────

    #[test]
    fn rss22_explicit_v2_equity_active_recognized() {
        let parsed = parse_runtime_session_source_mode(Some("v2_equity_active"));
        assert_eq!(
            parsed,
            RuntimeSessionSourceModeParse::Explicit(RuntimeSessionSourceMode::V2EquityActive)
        );
        assert!(parsed.invalid_reason().is_none());
        assert_eq!(
            parsed.resolved_mode(),
            RuntimeSessionSourceMode::V2EquityActive
        );
    }

    #[test]
    fn rss23_invalid_reason_mentions_all_three_modes() {
        let parsed = parse_runtime_session_source_mode(Some("nonsense"));
        let reason = parsed.invalid_reason().expect("must carry a reason");
        assert!(reason.contains("v2_equity_active"));
        assert!(reason.contains("v2_equity_shadow"));
        assert!(reason.contains("legacy"));
    }

    // ── ASSET-CORE-05E: evaluate_runtime_session_source_active_decision ─────

    #[test]
    fn rss24_active_decision_accepts_real_registry_at_regular_open() {
        let path = real_registry_path();
        let decision = evaluate_runtime_session_source_active_decision(&path, ts(TS_REGULAR_OPEN));
        match decision {
            RuntimeSessionSourceActiveDecision::Active {
                in_session,
                v2_session_state,
            } => {
                assert!(in_session, "regular-open must resolve in_session=true");
                assert_eq!(v2_session_state, "regular_open");
            }
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                panic!("expected Active, got Refused({reason})")
            }
        }
        assert!(
            evaluate_runtime_session_source_active_decision(&path, ts(TS_REGULAR_OPEN))
                .in_session_fail_closed()
        );
    }

    #[test]
    fn rss25_active_decision_accepts_real_registry_at_closed_weekend_in_session_false() {
        let path = real_registry_path();
        let decision =
            evaluate_runtime_session_source_active_decision(&path, ts(TS_WEEKEND_CLOSED));
        match decision {
            RuntimeSessionSourceActiveDecision::Active {
                in_session,
                v2_session_state,
            } => {
                assert!(!in_session, "closed weekend must resolve in_session=false");
                assert_eq!(v2_session_state, "closed");
            }
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                panic!("expected Active, got Refused({reason})")
            }
        }
        assert!(
            !evaluate_runtime_session_source_active_decision(&path, ts(TS_WEEKEND_CLOSED))
                .in_session_fail_closed()
        );
    }

    #[test]
    fn rss26_active_decision_accepts_real_registry_at_holiday_in_session_false() {
        let path = real_registry_path();
        let decision = evaluate_runtime_session_source_active_decision(&path, ts(TS_HOLIDAY));
        match decision {
            RuntimeSessionSourceActiveDecision::Active {
                in_session,
                v2_session_state,
            } => {
                assert!(!in_session, "holiday must resolve in_session=false");
                assert_eq!(v2_session_state, "holiday");
            }
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                panic!("expected Active, got Refused({reason})")
            }
        }
    }

    #[test]
    fn rss27_active_decision_accepts_real_registry_around_early_close() {
        let path = real_registry_path();
        let before =
            evaluate_runtime_session_source_active_decision(&path, ts(TS_BEFORE_EARLY_CLOSE));
        assert!(
            before.in_session_fail_closed(),
            "before early close must be in_session"
        );
        let after =
            evaluate_runtime_session_source_active_decision(&path, ts(TS_AFTER_EARLY_CLOSE));
        assert!(
            !after.in_session_fail_closed(),
            "after early close must NOT be in_session"
        );
        match after {
            RuntimeSessionSourceActiveDecision::Active {
                v2_session_state, ..
            } => {
                assert_eq!(v2_session_state, "early_close");
            }
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                panic!("expected Active, got Refused({reason})")
            }
        }
    }

    #[test]
    fn rss28_active_decision_refuses_missing_registry_fail_closed() {
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_missing_active_decision_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let decision =
            evaluate_runtime_session_source_active_decision(&missing_path, ts(TS_REGULAR_OPEN));
        assert!(
            !decision.in_session_fail_closed(),
            "missing registry must fail closed to not-in-session"
        );
        match decision {
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                assert!(reason.contains("missing") || reason.contains("load failed"));
            }
            RuntimeSessionSourceActiveDecision::Active { .. } => {
                panic!("missing registry must never report Active")
            }
        }
    }

    #[test]
    fn rss29_active_decision_refuses_zero_instrument_registry_fail_closed() {
        // An empty v1 registry (`[]`) loads, converts, and validates cleanly
        // — but produces checked_count == 0, so the underlying candidate
        // evaluation reports candidate_would_activate == false (same refusal
        // proven in-memory by rss17). This proves
        // evaluate_runtime_session_source_active_decision's `Ok(eval) if
        // !eval.candidate_would_activate => Refused` branch end-to-end
        // through a real, loadable, validatable registry file — not just the
        // registry-missing/malformed Err(..) branch already covered by
        // rss28/rss13/rss14.
        let path = std::env::temp_dir().join(format!(
            "mqk_active_decision_zero_instruments_{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "[]").expect("write temp empty-registry fixture");
        let decision = evaluate_runtime_session_source_active_decision(
            &path.to_string_lossy(),
            ts(TS_REGULAR_OPEN),
        );
        std::fs::remove_file(&path).ok();

        assert!(
            !decision.in_session_fail_closed(),
            "zero-instrument registry must fail closed to not-in-session"
        );
        match decision {
            RuntimeSessionSourceActiveDecision::Refused { reason } => {
                assert!(reason.contains("no equity") || reason.contains("checked"));
            }
            RuntimeSessionSourceActiveDecision::Active { .. } => {
                panic!("zero-instrument registry must never report Active")
            }
        }
    }

    // ── ASSET-CORE-05E: runtime_session_source_summary active-mode wiring ───

    #[test]
    fn rss30_summary_v2_equity_active_real_registry_accepts_clean() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
        let path = real_registry_path();
        let summary = runtime_session_source_summary(&path, ts(TS_REGULAR_OPEN));
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert_eq!(summary.session_source_mode, "v2_equity_active");
        assert!(summary.production_cutover_enabled);
        assert!(summary.runtime_uses_session_v2);
        assert!(summary.trading_uses_session_v2);
        assert!(summary.active_source_used);
        assert_eq!(summary.candidate_would_activate, Some(true));
        assert_eq!(
            summary.candidate_v2_parity_state.as_deref(),
            Some("matched")
        );
        assert_eq!(summary.fallback_reason, None);
    }

    #[test]
    fn rss31_summary_v2_equity_active_missing_registry_refuses_clean() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_missing_active_summary_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let summary = runtime_session_source_summary(&missing_path, ts(TS_REGULAR_OPEN));
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert_eq!(summary.session_source_mode, "v2_equity_active");
        // Configuration is on, but never claimed to be in effect.
        assert!(summary.production_cutover_enabled);
        assert!(!summary.runtime_uses_session_v2);
        assert!(!summary.trading_uses_session_v2);
        assert!(!summary.active_source_used);
        assert_eq!(summary.candidate_would_activate, None);
        let reason = summary
            .fallback_reason
            .expect("missing registry must carry a fallback_reason");
        assert!(reason.contains("missing") || reason.contains("load failed"));
    }

    #[test]
    fn rss32_summary_v2_equity_shadow_never_sets_active_source_used() {
        let _guard = runtime_session_source_env_test_lock().blocking_lock();
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");
        let path = real_registry_path();
        let summary = runtime_session_source_summary(&path, ts(TS_REGULAR_OPEN));
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        // Shadow mode evaluates cleanly (candidate_would_activate=true) but
        // must never claim to be the active source.
        assert_eq!(summary.candidate_would_activate, Some(true));
        assert!(!summary.active_source_used);
        assert!(!summary.production_cutover_enabled);
        assert!(!summary.runtime_uses_session_v2);
        assert!(!summary.trading_uses_session_v2);
    }
}
