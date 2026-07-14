//! SMART-CALENDAR-SESSION-PROVIDER-01: Market calendar and session provider seam.
//! NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01: Exchange-sourced provider seam.
//!
//! Adds a formal [`MarketCalendarProvider`] trait so autonomous session
//! controllers and readiness surfaces consume session truth through a
//! well-typed abstraction rather than raw string comparisons against
//! `CalendarSpec::NyseWeekdays.classify_market_session(ts)`.
//!
//! # Providers
//!
//! - [`NyseWeekdaysProvider`]: DST-aware, holiday-aware, early-close-aware
//!   heuristic provider.  Delegates to `mqk_integrity::CalendarSpec::NyseWeekdays`
//!   for all session classification (calendar logic lives in `mqk-integrity`).
//!   Source label: `"nyse_weekdays_heuristic"`.
//!
//! - [`FixedWindowOverrideProvider`]: wraps the env-var fixed-UTC-window override
//!   (`MQK_SESSION_START_HH_MM` / `MQK_SESSION_STOP_HH_MM`).  Does not consult
//!   any exchange calendar — the operator is responsible for configuring the
//!   window to match the exchange's DST period.  Source label:
//!   `"fixed_window_override"`.
//!
//! - [`ExchangeSourcedCalendarProvider`]: accepts injected exchange calendar data
//!   (fixture, file, or future API-backed source) and classifies session truth
//!   against that data.  Fails closed (`Unknown`) when source is unavailable,
//!   stale, or invalid — or falls back to the static heuristic provider when
//!   explicitly configured via [`ExchangeUnavailablePolicy::FallbackToStatic`].
//!   Source label determined by [`ExchangeCalendarMeta::source_name`]; fallback
//!   labeled `"exchange_sourced_fallback_to_static"`.
//!
//! # Fail-closed contract (SCSP07)
//!
//! `MarketSessionTruth::is_in_session()` returns `false` for every state except
//! `RegularOpen`.  `Unknown` is explicitly fail-closed: providers must return it
//! when truth cannot be determined, and callers must treat it as session-closed.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc, Weekday};
use mqk_integrity::{nyse_is_early_close_today, utc_to_et_components, CalendarSpec};

use super::session_controller::SessionWindow;

// ---------------------------------------------------------------------------
// MarketSessionState
// ---------------------------------------------------------------------------

/// Explicit market session state — SCSP-01.
///
/// Variants are ordered from most to least specific.  `Unknown` is the
/// fail-closed sentinel: callers must not treat it as session-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketSessionState {
    /// NYSE/Nasdaq regular session is open and the market is accepting orders.
    RegularOpen,
    /// Pre-market period (before 09:30 ET on a trading day).
    PreMarket,
    /// After-hours period (after 16:00 ET on a normal trading day).
    AfterHours,
    /// Market is closed — weekend or non-holiday weekday with no trading.
    Closed,
    /// Full-day exchange holiday (e.g., Christmas, Thanksgiving).
    Holiday,
    /// Market closed early; current time is past the shortened session close.
    EarlyClose,
    /// Provider could not determine session truth.  Treated as closed (SCSP07).
    Unknown,
}

impl MarketSessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RegularOpen => "regular_open",
            Self::PreMarket => "premarket",
            Self::AfterHours => "after_hours",
            Self::Closed => "closed",
            Self::Holiday => "holiday",
            Self::EarlyClose => "early_close",
            Self::Unknown => "unknown",
        }
    }

    /// `true` only for `RegularOpen`.
    ///
    /// Every other state — including `Unknown` — returns `false` (SCSP07).
    pub fn is_in_session(self) -> bool {
        self == Self::RegularOpen
    }
}

// ---------------------------------------------------------------------------
// MarketSessionTruth
// ---------------------------------------------------------------------------

/// Typed session truth returned by a [`MarketCalendarProvider`].
#[derive(Debug, Clone)]
pub struct MarketSessionTruth {
    /// The classified market session state at the queried time.
    pub state: MarketSessionState,
    /// Stable identifier for the provider that produced this truth.
    /// E.g., `"nyse_weekdays_heuristic"` or `"fixed_window_override"`.
    pub source: &'static str,
    /// Exchange whose calendar was consulted, or `"env_override"` for fixed windows.
    pub exchange: &'static str,
    /// `true` when the calendar day is a trading day, regardless of time-of-day.
    pub is_trading_day: bool,
    /// `true` when today is an early-close day (session shorter than normal).
    pub is_early_close: bool,
    /// Optional operator-facing note about session close time or reason.
    pub session_close_note: Option<&'static str>,
}

impl MarketSessionTruth {
    /// Fail-closed unknown truth — returned when the provider cannot determine session.
    pub fn unknown() -> Self {
        MarketSessionTruth {
            state: MarketSessionState::Unknown,
            source: "unknown",
            exchange: "unknown",
            is_trading_day: false,
            is_early_close: false,
            session_close_note: None,
        }
    }

    /// Delegating convenience: is the current moment a regular trading session?
    ///
    /// Fail-closed: `Unknown` and all non-regular states return `false` (SCSP07).
    pub fn is_in_session(&self) -> bool {
        self.state.is_in_session()
    }
}

// ---------------------------------------------------------------------------
// MarketCalendarProvider trait
// ---------------------------------------------------------------------------

/// Seam for market calendar and session truth — SCSP-01.
///
/// Implementations must be `Send + Sync` so they can be used across async
/// task boundaries or held in shared daemon state.
///
/// # Fail-closed contract (SCSP07)
///
/// When truth cannot be determined (out-of-range timestamp, internal error,
/// unconfigured provider), implementations must return
/// `MarketSessionTruth::unknown()`.  Callers must treat `Unknown` as closed.
pub trait MarketCalendarProvider: Send + Sync {
    /// Classify the market session at `now_utc`.
    fn session_for(&self, now_utc: DateTime<Utc>) -> MarketSessionTruth;
}

// ---------------------------------------------------------------------------
// NyseWeekdaysProvider
// ---------------------------------------------------------------------------

/// NYSE/Nasdaq regular-session provider — DST-aware, holiday-aware,
/// early-close-aware.
///
/// Delegates all session classification to
/// `mqk_integrity::CalendarSpec::NyseWeekdays`, which uses
/// `chrono_tz::America::New_York` for DST-correct conversion and a hardcoded
/// holiday + early-close table bounded to 2023–2028 (ASSET-CORE-05B audit;
/// previously misdocumented here as 2023–2026 — the underlying table in
/// `mqk-integrity` has covered 2023–2028 since
/// NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01). Dates outside this
/// bound are not represented and fall through to ordinary time-of-day
/// classification rather than failing closed — see `EQCAL01`/`EQCAL02` in
/// `scenario_market_calendar_session_provider_01.rs` for the exact bounded
/// coverage this provider can honestly claim.
///
/// Source label: `"nyse_weekdays_heuristic"`.  This is NOT exchange-sourced;
/// operator surfaces must label it as heuristic (SCSP07 honesty requirement).
pub struct NyseWeekdaysProvider;

impl MarketCalendarProvider for NyseWeekdaysProvider {
    fn session_for(&self, now_utc: DateTime<Utc>) -> MarketSessionTruth {
        let ts = now_utc.timestamp();
        let market_session = CalendarSpec::NyseWeekdays.classify_market_session(ts);
        let exchange_state = CalendarSpec::NyseWeekdays.classify_exchange_calendar(ts);

        // Determine whether today is an early-close day (ET-aware lookup).
        let is_ec = nyse_is_early_close_today(ts);

        let (state, is_trading_day) = match market_session {
            "regular" => (MarketSessionState::RegularOpen, true),
            "premarket" => (MarketSessionState::PreMarket, true),
            "after_hours" => {
                // On early-close days `nyse_classify_session` returns "after_hours"
                // once the shortened session ends (e.g., 13:00 ET).  Surface
                // EarlyClose so the operator can distinguish it from normal AH.
                let s = if is_ec {
                    MarketSessionState::EarlyClose
                } else {
                    MarketSessionState::AfterHours
                };
                (s, true)
            }
            "closed" => {
                let s = if exchange_state == "holiday" {
                    MarketSessionState::Holiday
                } else {
                    MarketSessionState::Closed
                };
                (s, false)
            }
            // Should not occur with CalendarSpec::NyseWeekdays; fail closed.
            _ => return MarketSessionTruth::unknown(),
        };

        MarketSessionTruth {
            state,
            source: "nyse_weekdays_heuristic",
            exchange: "NYSE",
            is_trading_day,
            is_early_close: is_ec,
            session_close_note: if is_ec {
                Some("early close 13:00 ET")
            } else {
                None
            },
        }
    }
}

// ---------------------------------------------------------------------------
// FixedWindowOverrideProvider
// ---------------------------------------------------------------------------

/// Fixed-UTC-window override provider.
///
/// Wraps a [`SessionWindow`] (HH:MM UTC bounds) and classifies the session by
/// checking whether `now_utc` falls within `[start, stop)`.  Does not consult
/// any exchange calendar; DST correctness is the operator's responsibility.
///
/// Used when `MQK_SESSION_START_HH_MM` and `MQK_SESSION_STOP_HH_MM` are both
/// set and valid.  Source label: `"fixed_window_override"`.
pub struct FixedWindowOverrideProvider {
    pub window: SessionWindow,
}

impl MarketCalendarProvider for FixedWindowOverrideProvider {
    fn session_for(&self, now_utc: DateTime<Utc>) -> MarketSessionTruth {
        let in_window = self.window.is_in_session(now_utc);
        let state = if in_window {
            MarketSessionState::RegularOpen
        } else {
            MarketSessionState::Closed
        };
        MarketSessionTruth {
            state,
            source: "fixed_window_override",
            exchange: "env_override",
            is_trading_day: in_window, // unknowable without calendar; approximate
            is_early_close: false,
            session_close_note: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Exchange-sourced calendar provider seam
// (NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01)
// ---------------------------------------------------------------------------

/// Per-day status from an exchange-sourced calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeDayStatus {
    /// Full-day exchange holiday — market closed all day.
    Holiday,
    /// Shortened session — market closes at [`ExchangeCalendarDay::early_close_et`].
    EarlyClose,
    /// Normal trading day — market open 09:30–16:00 ET.
    Open,
}

/// A single date entry in an exchange-sourced calendar.
///
/// Dates are expressed in Eastern Time (the NYSE/Nasdaq timezone).
/// Only holidays and early-close dates need to appear; unlisted weekdays
/// within the provider's coverage range are treated as normal open days.
#[derive(Debug, Clone)]
pub struct ExchangeCalendarDay {
    /// `(year, month, day)` in Eastern Time.
    pub date: (i64, i64, i64),
    /// Calendar status for this date.
    pub status: ExchangeDayStatus,
    /// For [`ExchangeDayStatus::EarlyClose`]: `(hour, minute)` ET close time.
    /// Ignored for other statuses.  Defaults to `(13, 0)` if `None`.
    pub early_close_et: Option<(u32, u32)>,
}

/// Provenance and coverage metadata for an exchange-sourced calendar dataset.
#[derive(Debug, Clone)]
pub struct ExchangeCalendarMeta {
    /// Stable identifier for the source (e.g., `"nyse_official_api_v1"`, `"fixture"`).
    /// Used as [`MarketSessionTruth::source`] when the provider is active.
    pub source_name: &'static str,
    /// Exchange whose calendar was loaded.
    pub exchange: &'static str,
    /// ISO8601 string indicating when this data was generated, if known.
    pub generated_at: Option<&'static str>,
    /// First date `(year, month, day)` covered by this dataset (ET, inclusive).
    pub coverage_start: (i64, i64, i64),
    /// Last date `(year, month, day)` covered by this dataset (ET, inclusive).
    pub coverage_end: (i64, i64, i64),
}

/// Source health state for an [`ExchangeSourcedCalendarProvider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeSourceState {
    /// Data is loaded and authoritative.
    Active,
    /// Data loaded but past configured freshness window.
    Stale,
    /// Data could not be parsed or is structurally invalid.
    Invalid,
    /// No data source has been configured or data could not be loaded.
    Unavailable,
}

/// What to do when the exchange source is not [`ExchangeSourceState::Active`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeUnavailablePolicy {
    /// Return `MarketSessionTruth::unknown()` — fail closed (SCSP07).
    FailClosed,
    /// Delegate to [`NyseWeekdaysProvider`] and relabel source as
    /// `"exchange_sourced_fallback_to_static"` so operators can distinguish
    /// this from a directly configured heuristic provider.
    FallbackToStatic,
}

/// Exchange-sourced calendar provider — accepts injected calendar data.
///
/// Classifies market sessions against per-day exchange calendar entries
/// (holiday, early-close, or normal open day) within a declared coverage
/// window.  Dates outside the coverage window fail closed (`Unknown`).
///
/// Fail-closed contract: when `source_state` is not `Active`, returns
/// `Unknown` (if `on_unavailable == FailClosed`) or delegates to the static
/// heuristic provider with an honest `"exchange_sourced_fallback_to_static"`
/// source label (if `on_unavailable == FallbackToStatic`).
///
/// No network calls are made — the caller is responsible for loading and
/// validating the `days` map before constructing this provider.
pub struct ExchangeSourcedCalendarProvider {
    /// Source provenance and coverage metadata.
    pub meta: ExchangeCalendarMeta,
    /// Current health of the underlying data source.
    pub source_state: ExchangeSourceState,
    /// Per-day calendar entries keyed by `(year, month, day)` in Eastern Time.
    pub days: HashMap<(i64, i64, i64), ExchangeCalendarDay>,
    /// Policy when source is not `Active`.
    pub on_unavailable: ExchangeUnavailablePolicy,
}

impl ExchangeSourcedCalendarProvider {
    fn in_coverage(&self, year: i64, month: i64, day: i64) -> bool {
        let date = (year, month, day);
        date >= self.meta.coverage_start && date <= self.meta.coverage_end
    }

    fn classify_open_day(
        &self,
        et_secs: i64,
        is_early_close: bool,
        ec_close_secs: i64,
    ) -> MarketSessionState {
        let open_secs = 9 * 3600 + 30 * 60; // 09:30 ET
        let close_secs = if is_early_close {
            ec_close_secs
        } else {
            16 * 3600 // 16:00 ET
        };
        if et_secs <= open_secs {
            MarketSessionState::PreMarket
        } else if et_secs <= close_secs {
            MarketSessionState::RegularOpen
        } else if is_early_close {
            MarketSessionState::EarlyClose
        } else {
            MarketSessionState::AfterHours
        }
    }
}

impl MarketCalendarProvider for ExchangeSourcedCalendarProvider {
    fn session_for(&self, now_utc: DateTime<Utc>) -> MarketSessionTruth {
        // If source is not active, apply the unavailability policy.
        if self.source_state != ExchangeSourceState::Active {
            return match self.on_unavailable {
                ExchangeUnavailablePolicy::FailClosed => MarketSessionTruth::unknown(),
                ExchangeUnavailablePolicy::FallbackToStatic => {
                    let base = NyseWeekdaysProvider.session_for(now_utc);
                    MarketSessionTruth {
                        source: "exchange_sourced_fallback_to_static",
                        ..base
                    }
                }
            };
        }

        // Decompose now_utc into Eastern Time components.
        let ts = now_utc.timestamp();
        let (year, month, day, et_secs, is_weekday) = match utc_to_et_components(ts) {
            Some(c) => c,
            None => return MarketSessionTruth::unknown(),
        };

        // Coverage check FIRST: dates outside the declared window are Unknown
        // (fail-closed) regardless of weekday/weekend.  We only have authoritative
        // truth for dates within coverage.
        if !self.in_coverage(year, month, day) {
            return MarketSessionTruth::unknown();
        }

        // Weekend within coverage: always Closed (exchange never trades Sat/Sun).
        if !is_weekday {
            return MarketSessionTruth {
                state: MarketSessionState::Closed,
                source: self.meta.source_name,
                exchange: self.meta.exchange,
                is_trading_day: false,
                is_early_close: false,
                session_close_note: None,
            };
        }

        // Look up this date in the per-day entries map.
        match self.days.get(&(year, month, day)) {
            Some(ExchangeCalendarDay {
                status: ExchangeDayStatus::Holiday,
                ..
            }) => MarketSessionTruth {
                state: MarketSessionState::Holiday,
                source: self.meta.source_name,
                exchange: self.meta.exchange,
                is_trading_day: false,
                is_early_close: false,
                session_close_note: None,
            },

            Some(ExchangeCalendarDay {
                status: ExchangeDayStatus::EarlyClose,
                early_close_et,
                ..
            }) => {
                let (ec_h, ec_m) = early_close_et.unwrap_or((13, 0));
                let ec_close_secs = (ec_h as i64) * 3600 + (ec_m as i64) * 60;
                let state = self.classify_open_day(et_secs, true, ec_close_secs);
                MarketSessionTruth {
                    state,
                    source: self.meta.source_name,
                    exchange: self.meta.exchange,
                    is_trading_day: true,
                    is_early_close: true,
                    session_close_note: Some("early close"),
                }
            }

            // ExchangeDayStatus::Open or not in map → normal trading day.
            _ => {
                let state = self.classify_open_day(et_secs, false, 0);
                MarketSessionTruth {
                    state,
                    source: self.meta.source_name,
                    exchange: self.meta.exchange,
                    is_trading_day: true,
                    is_early_close: false,
                    session_close_note: None,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ASSET-CORE-05A: Multi-asset session-classification seam (model-only)
// ---------------------------------------------------------------------------
//
// The types and functions below are an ADDITIVE classification seam for
// representing session concepts across future asset classes (crypto,
// futures, forex) alongside the existing equity-only `MarketSessionState`.
//
// Honesty contract:
// - This seam does NOT replace `MarketCalendarProvider`, `MarketSessionState`,
//   or any equity runtime behavior. `NyseWeekdaysProvider`,
//   `FixedWindowOverrideProvider`, and `ExchangeSourcedCalendarProvider`
//   above are unchanged and remain the only providers consulted by
//   `session_controller` / `start_execution_runtime`.
// - Only `MarketSessionProfile::EquityUsRegular` is backed by real exchange
//   calendar logic (`CalendarSpec::NyseWeekdays`, hardcoded holiday/early-close
//   tables). The crypto/futures/forex classifiers below are MODEL ONLY: they
//   represent the *shape* of session truth for those asset classes (e.g.
//   "continuous," "weekday vs weekend," "regular vs extended vs overnight")
//   but do not consult any authoritative exchange or holiday calendar. No
//   CME holiday table, no FX market-center calendar, and no crypto
//   exchange-specific downtime is modeled here.
// - Nothing in this section is called from `session_controller.rs`,
//   `start_execution_runtime`, or any route. Wiring a new asset class into
//   the runtime is explicit future work (ASSET-CORE-05B+), not a side
//   effect of this seam existing.

/// Generic, asset-class-agnostic session kind — ASSET-CORE-05A.
///
/// Where [`MarketSessionState`] is equity-specific (NYSE/Nasdaq terminology),
/// `MarketVenueSessionKind` is the additive generalization that future
/// asset-class session profiles classify into. Variants are a superset
/// covering equities, crypto, futures, and forex session shapes.
///
/// # Fail-closed contract
///
/// [`MarketVenueSessionKind::is_open`] returns `true` only for `Regular` and
/// `Continuous`. Every other variant — including `Extended`, `Overnight`,
/// and `Unknown` — returns `false`. This intentionally does NOT assert that
/// futures `Extended`/`Overnight` sessions are tradable; that policy
/// decision is out of scope for this model-only seam and is future work
/// if/when a futures asset class is actually enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketVenueSessionKind {
    /// Venue is closed; no trading activity expected.
    Closed,
    /// Pre-regular-session period (e.g., equity premarket).
    PreMarket,
    /// The venue's primary/regular trading session.
    Regular,
    /// Post-regular-session period (e.g., equity after-hours).
    PostMarket,
    /// Extended session immediately surrounding regular hours (e.g.,
    /// futures Globex extended hours).
    Extended,
    /// Deep overnight session, outside both regular and extended windows.
    Overnight,
    /// No session boundary exists — venue trades continuously (e.g., crypto
    /// 24/7, forex 24/5 weekdays).
    Continuous,
    /// Classification could not be determined. Treated as closed.
    Unknown,
}

impl MarketVenueSessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::PreMarket => "premarket",
            Self::Regular => "regular",
            Self::PostMarket => "post_market",
            Self::Extended => "extended",
            Self::Overnight => "overnight",
            Self::Continuous => "continuous",
            Self::Unknown => "unknown",
        }
    }

    /// `true` only for `Regular` and `Continuous` (fail-closed contract).
    ///
    /// `Extended` and `Overnight` deliberately return `false`: this seam
    /// does not assert futures extended-hours tradability, which is a
    /// policy decision for a future patch, not this model-only seam.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Regular | Self::Continuous)
    }
}

impl MarketSessionState {
    /// Maps the existing equity-only state onto the generic
    /// [`MarketVenueSessionKind`] model — ASSET-CORE-05A.
    ///
    /// Purely additive: does not change `MarketSessionState` itself or any
    /// caller of `is_in_session()`. `Holiday` and `Closed` both map to
    /// `Closed`; `EarlyClose` maps to `PostMarket` (the session has already
    /// ended for the day, same as ordinary after-hours).
    pub fn to_venue_kind(self) -> MarketVenueSessionKind {
        match self {
            Self::RegularOpen => MarketVenueSessionKind::Regular,
            Self::PreMarket => MarketVenueSessionKind::PreMarket,
            Self::AfterHours => MarketVenueSessionKind::PostMarket,
            Self::Closed => MarketVenueSessionKind::Closed,
            Self::Holiday => MarketVenueSessionKind::Closed,
            Self::EarlyClose => MarketVenueSessionKind::PostMarket,
            Self::Unknown => MarketVenueSessionKind::Unknown,
        }
    }
}

/// Named session profile for a future asset class — ASSET-CORE-05A.
///
/// Distinct from [`MarketSessionState`] (equity-only) and
/// [`MarketVenueSessionKind`] (generic kind): a `MarketSessionProfile`
/// identifies *which* classifier/calendar shape applies to a given asset
/// class, independent of the specific moment-in-time classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketSessionProfile {
    /// US equities regular session — backed by real NYSE calendar logic.
    EquityUsRegular,
    /// Crypto 24/7 continuous session — model only.
    CryptoContinuous,
    /// Futures Globex-style regular/extended/overnight session — model only.
    FuturesGlobex,
    /// Forex 24/5 weekday-continuous session — model only.
    ForexWeekdayContinuous,
}

impl MarketSessionProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EquityUsRegular => "equity_us_regular",
            Self::CryptoContinuous => "crypto_continuous",
            Self::FuturesGlobex => "futures_globex",
            Self::ForexWeekdayContinuous => "forex_24x5",
        }
    }

    /// Honest implementation-status label for this profile.
    ///
    /// `"implemented_current"`: backed by real exchange-calendar logic and
    /// already used by production session truth (equity only).
    /// `"model_only"`: classifiable by this seam's pure functions, but not
    /// backed by any authoritative calendar and not wired into any runtime
    /// or trading decision.
    pub fn implementation_status(self) -> &'static str {
        match self {
            Self::EquityUsRegular => "implemented_current",
            Self::CryptoContinuous | Self::FuturesGlobex | Self::ForexWeekdayContinuous => {
                "model_only"
            }
        }
    }
}

/// Authority level for a session-profile status answer.
///
/// This is diagnostic metadata only. It is not consumed by any trading,
/// admission, risk, broker, or runtime path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAuthority {
    /// Backed by a source that can truthfully answer for the requested profile.
    Authoritative,
    /// Known model or heuristic, but not a complete exchange-specific calendar.
    Fallback,
    /// Explicit operator/configured override rather than exchange calendar truth.
    ConfiguredOverride,
    /// No trustworthy answer is available.
    Unavailable,
}

impl SessionAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::Fallback => "fallback",
            Self::ConfiguredOverride => "configured_override",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Read-only status for the active session profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProfileStatus {
    pub profile: MarketSessionProfile,
    pub authority: SessionAuthority,
    pub is_open: Option<bool>,
    pub reason_code: &'static str,
    pub message: &'static str,
}

/// Profiles that the current model can name.
///
/// Only `equity_us_regular` is wired into current session/trading behavior.
/// The other entries are diagnostic/model-only scaffolds.
pub fn supported_session_profiles() -> Vec<MarketSessionProfile> {
    vec![
        MarketSessionProfile::EquityUsRegular,
        MarketSessionProfile::CryptoContinuous,
        MarketSessionProfile::FuturesGlobex,
        MarketSessionProfile::ForexWeekdayContinuous,
    ]
}

/// Classify the US equity regular-session profile — ASSET-CORE-05A.
///
/// Thin wrapper over [`MarketSessionState::to_venue_kind`] so callers can
/// classify equity sessions through the same generic shape as the
/// model-only profiles below. Backed by real exchange-calendar logic via
/// whichever [`MarketCalendarProvider`] produced `state`.
pub fn classify_equity_us_regular_session(state: MarketSessionState) -> MarketVenueSessionKind {
    state.to_venue_kind()
}

/// Classify the crypto 24/7 continuous session profile — MODEL ONLY
/// (ASSET-CORE-05A).
///
/// Crypto venues have no exchange-calendar concept of a closed day; every
/// moment is tradable. Takes `_now_utc` for symmetry with the other profile
/// classifiers even though the result never varies — this makes the
/// "always continuous" claim explicit and testable rather than implicit.
/// Not wired into any runtime path; crypto remains disabled.
pub fn classify_crypto_continuous_session(_now_utc: DateTime<Utc>) -> MarketVenueSessionKind {
    MarketVenueSessionKind::Continuous
}

/// Classify the forex 24/5 weekday-continuous session profile — MODEL ONLY
/// (ASSET-CORE-05A).
///
/// Concept-level only: Monday-Friday is `Continuous`, Saturday/Sunday is
/// `Closed`. Does not model the precise Friday-evening-to-Sunday-evening ET
/// rollover used by real FX market centers — that refinement is future work
/// if/when a forex asset class is actually enabled. Not wired into any
/// runtime path; forex remains disabled.
pub fn classify_forex_weekday_continuous_session(now_utc: DateTime<Utc>) -> MarketVenueSessionKind {
    match now_utc.weekday() {
        Weekday::Sat | Weekday::Sun => MarketVenueSessionKind::Closed,
        _ => MarketVenueSessionKind::Continuous,
    }
}

/// Caller-supplied UTC HH:MM window bounds for the futures Globex-style
/// classifier — MODEL ONLY (ASSET-CORE-05A).
///
/// Reuses [`SessionWindow`] (the same fixed-UTC-window type backing
/// `FixedWindowOverrideProvider`) for both bounds rather than introducing a
/// parallel HH:MM representation. `extended` must be a superset of
/// `regular` for [`classify_futures_globex_session`] to behave sensibly,
/// but this is not enforced — the caller owns boundary correctness, same as
/// `FixedWindowOverrideProvider`'s env-sourced window today.
///
/// This slice does not hardcode any real CME (or other futures exchange)
/// product's actual trading schedule, and — because `SessionWindow` cannot
/// express a window that wraps midnight — cannot represent the real
/// Sunday-evening-to-Friday-evening Globex rollover. The caller supplies
/// illustrative or product-specific same-day boundaries. No holiday
/// calendar is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuturesSessionWindows {
    /// Regular trading hours, UTC HH:MM bounds.
    pub regular: SessionWindow,
    /// Extended session bounds, UTC HH:MM bounds.
    pub extended: SessionWindow,
}

/// Classify the futures Globex-style session profile — MODEL ONLY
/// (ASSET-CORE-05A).
///
/// Distinguishes `Regular` (inside `windows.regular`) from `Extended`
/// (inside `windows.extended` but outside `regular`) from `Overnight`
/// (outside both windows) from `Closed` (Saturday, treated as the one day
/// with no Globex-style activity in this model-only slice — real Globex
/// Sunday-evening opens are approximated as `Overnight` rather than
/// asserting a precise open time). No CME holiday calendar is consulted —
/// full-day closures other than Saturday are not modeled here. Not wired
/// into any runtime path; futures remain disabled.
pub fn classify_futures_globex_session(
    now_utc: DateTime<Utc>,
    windows: &FuturesSessionWindows,
) -> MarketVenueSessionKind {
    if now_utc.weekday() == Weekday::Sat {
        return MarketVenueSessionKind::Closed;
    }
    if windows.regular.is_in_session(now_utc) {
        MarketVenueSessionKind::Regular
    } else if windows.extended.is_in_session(now_utc) {
        MarketVenueSessionKind::Extended
    } else {
        MarketVenueSessionKind::Overnight
    }
}

// ---------------------------------------------------------------------------
// ASSET-CORE-05B: Session-profile resolution seam (instrument metadata, read-only)
// ---------------------------------------------------------------------------
//
// Resolves an instrument's `asset_class` (+ optional `instrument_kind`)
// string metadata to the [`MarketSessionProfile`] that *would* govern its
// session truth, using the same canonical asset-class strings already
// established by `mqk-md` (`instrument_registry.rs` / `instrument_registry_v2.rs`):
// `"equity"` (with `instrument_kind = Some("etf")` for ETFs — ETFs are not a
// distinct asset class in this repo), `"crypto"`, `"future"`/`"futures"`,
// `"forex"`, `"option"`/`"options"`. Matching is exact-case, mirroring
// `mqk-md::provider_registry::asset_class_from_config`'s existing convention
// (no case-folding) — every canonical asset_class value written by this
// repo's registries is already lowercase.
//
// Honesty contract:
// - Pure function: no DB, no config/env var, no network, no wall clock.
// - Does not make any non-equity asset class tradable. A returned
//   `MarketSessionProfile` here is metadata classification only — this
//   function is not called by `session_controller.rs`,
//   `start_execution_runtime`, any route, or any gate.
// - Only equity/ETF resolves to `SessionProfileResolutionTruth::Active`
//   (the real, currently-wired session authority). Crypto/futures/forex
//   resolve to `UnsupportedAssetClass` carrying their model-only
//   `MarketSessionProfile` from ASSET-CORE-05A, so callers can see *which*
//   shape would apply without it being wired up anywhere.
// - Options resolve to `UnsupportedAssetClass` with `profile: None`: options
//   have no independent session-profile shape modeled in this seam. They
//   will likely inherit the underlying equity session in a future patch;
//   that inheritance is not invented here.
// - Unknown and blank/whitespace asset classes fail closed to `Unknown`.

/// Resolution outcome for [`resolve_session_profile_for_instrument_metadata`] — ASSET-CORE-05B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProfileResolutionTruth {
    /// `profile` is the real, currently-wired session authority for this metadata.
    Active,
    /// `asset_class` is recognized, but its session profile (if any) is
    /// model-only and not wired into any runtime or trading path.
    UnsupportedAssetClass,
    /// `asset_class` could not be classified. Fail-closed: callers must not
    /// treat this as any particular session shape.
    Unknown,
}

/// Pure resolution result returned by
/// [`resolve_session_profile_for_instrument_metadata`] — ASSET-CORE-05B.
///
/// `profile` is `None` whenever no session-profile shape applies (unknown
/// asset class, or options — see module docs above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionProfileResolution {
    pub truth_state: SessionProfileResolutionTruth,
    pub profile: Option<MarketSessionProfile>,
    pub reason: &'static str,
}

/// Resolve `(asset_class, instrument_kind)` instrument metadata to the
/// session profile that would classify it — ASSET-CORE-05B.
///
/// Pure, read-only metadata classification; see the module docs above for
/// the full honesty contract. Accepts both the singular (`"future"`,
/// `"option"`) and plural (`"futures"`, `"options"`) spellings used across
/// this repo's provider/registry layers (`mqk-md::provider_registry` uses
/// plural; `mqk-md::instrument_registry_v2` uses singular).
pub fn resolve_session_profile_for_instrument_metadata(
    asset_class: &str,
    instrument_kind: Option<&str>,
) -> SessionProfileResolution {
    let trimmed = asset_class.trim();
    if trimmed.is_empty() {
        return SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::Unknown,
            profile: None,
            reason: "blank asset_class",
        };
    }

    match trimmed {
        "equity" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::Active,
            profile: Some(MarketSessionProfile::EquityUsRegular),
            reason: if instrument_kind == Some("etf") {
                "etf trades on the equity regular session"
            } else {
                "equity regular session is the real, wired session authority"
            },
        },
        "crypto" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::UnsupportedAssetClass,
            profile: Some(MarketSessionProfile::CryptoContinuous),
            reason: "crypto continuous profile is model-only and not wired into runtime",
        },
        "future" | "futures" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::UnsupportedAssetClass,
            profile: Some(MarketSessionProfile::FuturesGlobex),
            reason: "futures globex-style profile is model-only and not wired into runtime",
        },
        "forex" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::UnsupportedAssetClass,
            profile: Some(MarketSessionProfile::ForexWeekdayContinuous),
            reason: "forex 24x5 profile is model-only and not wired into runtime",
        },
        "option" | "options" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::UnsupportedAssetClass,
            profile: None,
            reason: "options have no modeled session profile in this seam; likely to inherit the underlying equity session in a future patch",
        },
        // ASSET-CORE-05H: "rate" is a recognized, schema-valid asset class
        // (mqk_md::instrument_registry_v2::CANONICAL_ASSET_CLASSES_V2), not a
        // truly unrecognized string — it must not fall through to the
        // `Unknown` catch-all below. No session-profile shape is modeled for
        // fixed-income/rates instruments in this seam (no `MarketSessionProfile`
        // variant exists for them), so this reports the same honest
        // `UnsupportedAssetClass` + `profile: None` shape as `"option"`/`"options"`
        // rather than inventing a new profile.
        "rate" => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::UnsupportedAssetClass,
            profile: None,
            reason: "rate/fixed-income instruments have no modeled session profile in this seam",
        },
        _ => SessionProfileResolution {
            truth_state: SessionProfileResolutionTruth::Unknown,
            profile: None,
            reason: "unrecognized asset_class",
        },
    }
}

// ---------------------------------------------------------------------------
// ASSET-CORE-05H: pure, composed per-instrument session router
// ---------------------------------------------------------------------------
//
// Composes the ASSET-CORE-05B profile resolver above with the per-profile
// session-state classification ASSET-CORE-05B/05C already perform inline at
// each call site (`mqk-daemon/src/routes/system.rs`), so callers get one
// pure, reusable, independently-testable entry point instead of duplicating
// the two-step resolve-then-classify sequence. Not called by any trading,
// admission, risk, broker, or runtime path — read-only diagnostic composition
// only, same honesty contract as `resolve_session_profile_for_instrument_metadata`.

/// Session-state classification for a resolved [`MarketSessionProfile`] at a
/// given instant — ASSET-CORE-05B, relocated here (was a private fn in
/// `routes/system.rs`) so [`route_instrument_session_for_metadata`] can reuse
/// it without duplicating its match arms. Equity/ETF is classified via the
/// real, production-wired [`NyseWeekdaysProvider`]; crypto/futures/forex use
/// the model-only classifiers above (futures against a static illustrative
/// [`FuturesSessionWindows`] fixture — no real CME calendar is consulted).
/// Returns `(session_state, reason_code)`.
pub fn instrument_session_state_for_profile(
    profile: Option<MarketSessionProfile>,
    now_utc: DateTime<Utc>,
) -> (&'static str, &'static str) {
    match profile {
        Some(MarketSessionProfile::EquityUsRegular) => {
            let equity_truth = NyseWeekdaysProvider.session_for(now_utc);
            let _venue_kind = classify_equity_us_regular_session(equity_truth.state);
            (
                equity_truth.state.as_str(),
                "classified_by_market_calendar_provider",
            )
        }
        Some(MarketSessionProfile::CryptoContinuous) => (
            classify_crypto_continuous_session(now_utc).as_str(),
            "classified_by_model_only_crypto_continuous",
        ),
        Some(MarketSessionProfile::FuturesGlobex) => {
            let windows = FuturesSessionWindows {
                regular: SessionWindow::parse("14:30", "21:00")
                    .expect("static futures regular window must parse"),
                extended: SessionWindow::parse("12:00", "23:00")
                    .expect("static futures extended window must parse"),
            };
            (
                classify_futures_globex_session(now_utc, &windows).as_str(),
                "classified_by_model_only_futures_fixture_windows",
            )
        }
        Some(MarketSessionProfile::ForexWeekdayContinuous) => (
            classify_forex_weekday_continuous_session(now_utc).as_str(),
            "classified_by_model_only_forex_weekday_continuous",
        ),
        None => (
            MarketVenueSessionKind::Unknown.as_str(),
            "session_profile_unavailable",
        ),
    }
}

/// Result of routing one instrument's `(asset_class, instrument_kind)`
/// metadata through the full ASSET-CORE-05B/05H session-routing pipeline at
/// `now_utc` — ASSET-CORE-05H.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentSessionRoute {
    /// The profile-resolution step (ASSET-CORE-05B).
    pub profile_resolution: SessionProfileResolution,
    /// The classified session state for the resolved profile at `now_utc`
    /// (e.g. `"regular_open"`, `"continuous"`, `"closed"`, `"unknown"`).
    pub session_state: &'static str,
    /// Reason code for the session-state classification step.
    pub session_reason_code: &'static str,
}

impl InstrumentSessionRoute {
    /// `true` only when the resolved profile is the real, currently-wired
    /// equity session authority (mirrors the `production_backed` flag each
    /// route handler in `routes/system.rs` computes independently today).
    pub fn is_production_backed(&self) -> bool {
        self.profile_resolution.truth_state == SessionProfileResolutionTruth::Active
            && self.profile_resolution.profile == Some(MarketSessionProfile::EquityUsRegular)
    }

    /// `true` when the resolved profile is a recognized-but-unwired
    /// non-equity model (crypto/futures/forex). `false` for equity, `false`
    /// for `Unknown`/unsupported-with-no-profile (e.g. options, rate).
    pub fn is_model_only(&self) -> bool {
        self.profile_resolution.truth_state == SessionProfileResolutionTruth::UnsupportedAssetClass
            && self.profile_resolution.profile.is_some()
    }
}

/// Route one instrument's metadata to its full session decision — pure,
/// deterministic, composed from [`resolve_session_profile_for_instrument_metadata`]
/// and [`instrument_session_state_for_profile`] — ASSET-CORE-05H.
///
/// No DB, config, env, network, provider, broker, runtime, OMS, or risk
/// access. `enabled`/`paper_trading_enabled`/`live_trading_enabled` are not
/// parameters — this seam has no enablement concept and never changes
/// classification based on them (callers must not read "unsupported"/
/// "unknown" here as any comment on whether an instrument is or should be
/// tradable).
pub fn route_instrument_session_for_metadata(
    asset_class: &str,
    instrument_kind: Option<&str>,
    now_utc: DateTime<Utc>,
) -> InstrumentSessionRoute {
    let profile_resolution =
        resolve_session_profile_for_instrument_metadata(asset_class, instrument_kind);
    let (session_state, session_reason_code) =
        instrument_session_state_for_profile(profile_resolution.profile, now_utc);
    InstrumentSessionRoute {
        profile_resolution,
        session_state,
        session_reason_code,
    }
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED §6a: typed market-session
// schedule seam.
// ---------------------------------------------------------------------------
//
// `MarketCalendarProvider::session_for()` answers "what state is `now_utc`
// in" — it does not say what today's session open/close/previous-trading-date
// are. `MarketSessionSchedule` composes over the existing provider (walking
// day-by-day via its own `is_trading_day` classification — not a second
// calendar implementation) to answer those questions, plus imposes its own
// explicit coverage-window fail-closed check on top of whatever the
// underlying provider returns (the static `NyseWeekdaysProvider` heuristic
// does not itself fail closed outside its hardcoded 2023-2028 table).

/// Coverage/authority state for a resolved [`MarketSessionSchedule`].
///
/// `Active` is the only state callers may treat as authoritative for
/// continuity/expected-bar computation. Every other state is fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarCoverageState {
    /// Date is within this seam's declared coverage window and the
    /// underlying provider produced a determinate (non-`Unknown`) session
    /// state for it.
    Active,
    /// Reserved for a future provider that can report staleness explicitly;
    /// no current provider composed by this seam produces this state.
    Stale,
    /// Reserved for a future provider that can report structural invalidity;
    /// no current provider composed by this seam produces this state.
    Invalid,
    /// Date falls outside this seam's declared coverage window.
    OutOfRange,
    /// The underlying provider could not determine session truth for this
    /// instant, or is a non-calendar-backed override
    /// ([`FixedWindowOverrideProvider`]) that cannot honestly answer
    /// holiday/previous-trading-date questions.
    Unknown,
}

/// This seam's own declared coverage window (ET calendar dates, inclusive),
/// independent of whatever the underlying [`MarketCalendarProvider`] claims.
/// Mirrors `NyseWeekdaysProvider`'s documented 2023-2028 holiday/early-close
/// table bound (`mqk-integrity::calendar`) — the same real bound the static
/// heuristic provider can honestly claim, imposed explicitly here because the
/// provider itself does not fail closed outside it.
pub const SCHEDULE_COVERAGE_START: (i64, i64, i64) = (2023, 1, 1);
pub const SCHEDULE_COVERAGE_END: (i64, i64, i64) = (2028, 12, 31);

/// Typed market-session schedule for one ET calendar date — §6a.
#[derive(Debug, Clone)]
pub struct MarketSessionSchedule {
    /// `(year, month, day)` in Eastern Time — the date `now_utc` classified into.
    pub market_date: (i64, i64, i64),
    /// 09:30 ET, DST-correct, converted to UTC.
    pub session_open_utc: DateTime<Utc>,
    /// 16:00 ET (or the early-close time), DST-correct, converted to UTC.
    pub session_close_utc: DateTime<Utc>,
    /// The most recent prior trading date, found by walking backward through
    /// the provider's own `is_trading_day` classification (real calendar
    /// walk, not a fixed day-count).
    pub previous_trading_date: (i64, i64, i64),
    pub is_early_close: bool,
    /// `true` when `market_date` itself is a trading day (mirrors
    /// [`MarketSessionTruth::is_trading_day`]) — `false` for a weekend or
    /// full-day exchange holiday, regardless of the wall-clock time on that
    /// date (DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01, REPAIR 1). Callers
    /// computing an intraday expected-bar grid must check this before
    /// treating `session_open_utc`/`session_close_utc` as a real session —
    /// those fields are still populated with the ordinary-hours arithmetic
    /// even on a non-trading day, since they are only meaningful once this
    /// flag is checked.
    pub is_trading_day: bool,
    /// Mirrors [`MarketSessionTruth::source`] for the instant classified.
    pub calendar_source: &'static str,
    pub coverage_state: CalendarCoverageState,
}

fn within_schedule_coverage(date: (i64, i64, i64)) -> bool {
    date >= SCHEDULE_COVERAGE_START && date <= SCHEDULE_COVERAGE_END
}

/// Days since the Unix epoch for a civil `(year, month, day)` date.
/// Howard Hinnant's `days_from_civil` algorithm — deterministic, no stdlib
/// date dependency, exact inverse of `mqk_integrity::epoch_secs_to_ymd`.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (month + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Midnight UTC (00:00:00Z) epoch-seconds for a civil `(year, month, day)`
/// date — the expected daily `md_bars.end_ts` label (Alpaca stores the
/// bar-start `t` directly as `end_ts`; daily bars are requested with a
/// `T00:00:00Z` start boundary — see `mqk-md::alpaca_provider`).
pub fn midnight_utc_ts_for_date(date: (i64, i64, i64)) -> i64 {
    days_from_civil(date.0, date.1, date.2) * 86_400
}

/// A stable mid-day UTC anchor (17:00 UTC) for a civil date — comfortably
/// inside any DST offset's daytime hours, so `is_trading_day`/holiday
/// classification for that date is never ambiguous near a UTC-day boundary.
/// Used only for calendar-classification purposes, never as a bar-timestamp
/// label.
fn midday_utc_anchor_ts_for_date(date: (i64, i64, i64)) -> i64 {
    midnight_utc_ts_for_date(date) + 17 * 3600
}

/// Walk backward from (and including) `start_date`, day-by-day, through
/// `provider`'s own `is_trading_day` classification, collecting `count`
/// trading dates. Returns them in ascending order (oldest first,
/// `start_date`-or-earlier last... no: most recent last).
///
/// Bounded to `count * 3 + 30` calendar-day steps — comfortably covers any
/// real NYSE holiday/long-weekend cluster for the small `count` values this
/// evaluator uses (at most 21).
///
/// Returns `None` if `count` trading dates are not found within the bound —
/// callers must treat this as calendar-unavailable, never a silent partial
/// window. Also returns `None` the moment the walk steps outside this seam's
/// declared `SCHEDULE_COVERAGE_START..=SCHEDULE_COVERAGE_END` window
/// (DAILY-DATA-READINESS-01B-CLOSURE-REPAIR-01, REPAIR 2): the underlying
/// static `NyseWeekdaysProvider` does not itself fail closed outside its
/// 2023-2028 table (module doc above) — it silently falls through to
/// ordinary weekday classification — so this walk must not trust that
/// fallback for any date in a bounded warmup window, not merely the
/// evaluation instant's own date.
pub fn walk_back_trading_dates(
    provider: &dyn MarketCalendarProvider,
    start_date: (i64, i64, i64),
    count: usize,
) -> Option<Vec<(i64, i64, i64)>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let mut found: Vec<(i64, i64, i64)> = Vec::with_capacity(count);
    let mut candidate = start_date;
    let max_steps = count * 3 + 30;
    for _ in 0..max_steps {
        if !within_schedule_coverage(candidate) {
            return None;
        }
        let ts = midday_utc_anchor_ts_for_date(candidate);
        let dt = DateTime::<Utc>::from_timestamp(ts, 0)?;
        let truth = provider.session_for(dt);
        if truth.is_trading_day {
            found.push(candidate);
            if found.len() == count {
                found.reverse(); // ascending: oldest first
                return Some(found);
            }
        }
        let prev_ts = ts - 86_400;
        let (year, month, day, _, _) = utc_to_et_components(prev_ts)?;
        candidate = (year, month, day);
    }
    None
}

/// Walk backward from `now_utc` (day-by-day, UTC-day steps) through
/// `provider`'s own `is_trading_day` classification to find the most recent
/// prior trading date (strictly before `now_utc`'s own ET date).
///
/// Returns `None` if no trading day is found within the bound — callers must
/// treat `None` as calendar-unavailable, not silently fall back to `market_date`.
fn find_previous_trading_date(
    provider: &dyn MarketCalendarProvider,
    now_utc: DateTime<Utc>,
) -> Option<(i64, i64, i64)> {
    let (year, month, day, _, _) = utc_to_et_components(now_utc.timestamp())?;
    let day_before_ts = midnight_utc_ts_for_date((year, month, day)) - 1;
    let (py, pm, pd, _, _) = utc_to_et_components(day_before_ts)?;
    walk_back_trading_dates(provider, (py, pm, pd), 1).map(|dates| dates[0])
}

/// [`resolve_market_session_schedule`] for an explicit civil date rather than
/// a `now_utc` instant — used to resolve a *prior* session's own
/// open/close/grid without needing the wall clock to land inside it.
/// Anchors the underlying provider evaluation at [`midday_utc_anchor_ts_for_date`]
/// so calendar classification is never ambiguous near a UTC-day boundary.
pub fn resolve_market_session_schedule_for_date(
    provider: &dyn MarketCalendarProvider,
    date: (i64, i64, i64),
) -> Option<MarketSessionSchedule> {
    let ts = midday_utc_anchor_ts_for_date(date);
    let dt = DateTime::<Utc>::from_timestamp(ts, 0)?;
    Some(resolve_market_session_schedule(provider, dt))
}

/// Resolve the [`MarketSessionSchedule`] for `now_utc` against `provider`.
///
/// Pure composition: no DB, network, or broker access. `provider` is
/// evaluated at `now_utc` for `market_date`'s classification, and at up to
/// 10 prior UTC-day steps for [`MarketSessionSchedule::previous_trading_date`].
///
/// `session_open_utc`/`session_close_utc` are derived from `now_utc`'s own
/// ET offset (`now_utc.timestamp() - et_secs` gives the UTC instant of ET
/// midnight under the offset in effect at `now_utc`), which is exact for
/// every instant except the ~1-hour DST-transition window itself (transition
/// occurs at 02:00 ET; any evaluation during ordinary trading hours is
/// unaffected).
pub fn resolve_market_session_schedule(
    provider: &dyn MarketCalendarProvider,
    now_utc: DateTime<Utc>,
) -> MarketSessionSchedule {
    let et_components = utc_to_et_components(now_utc.timestamp());
    let Some((year, month, day, et_secs, _is_weekday)) = et_components else {
        return MarketSessionSchedule {
            market_date: (0, 0, 0),
            session_open_utc: now_utc,
            session_close_utc: now_utc,
            previous_trading_date: (0, 0, 0),
            is_early_close: false,
            is_trading_day: false,
            calendar_source: "unknown",
            coverage_state: CalendarCoverageState::Unknown,
        };
    };
    let market_date = (year, month, day);
    let truth = provider.session_for(now_utc);

    let midnight_et_utc_ts = now_utc.timestamp() - et_secs;
    let open_secs = 9 * 3600 + 30 * 60; // 09:30 ET
    let close_secs = if truth.is_early_close {
        13 * 3600 // 13:00 ET early close
    } else {
        16 * 3600 // 16:00 ET regular close
    };
    let session_open_utc =
        DateTime::<Utc>::from_timestamp(midnight_et_utc_ts + open_secs, 0).unwrap_or(now_utc);
    let session_close_utc =
        DateTime::<Utc>::from_timestamp(midnight_et_utc_ts + close_secs, 0).unwrap_or(now_utc);

    let previous_trading_date = find_previous_trading_date(provider, now_utc);

    // FixedWindowOverrideProvider consults no exchange calendar at all — it
    // cannot honestly answer holiday/previous-trading-date questions, so
    // this seam must not treat it as calendar truth for those purposes
    // (contract §6a), regardless of what state it reports for `now_utc`.
    let is_fixed_window_override = truth.source == "fixed_window_override";

    let coverage_state = if is_fixed_window_override || truth.state == MarketSessionState::Unknown {
        CalendarCoverageState::Unknown
    } else if !within_schedule_coverage(market_date) {
        CalendarCoverageState::OutOfRange
    } else if previous_trading_date.is_none() {
        CalendarCoverageState::Unknown
    } else {
        CalendarCoverageState::Active
    };

    MarketSessionSchedule {
        market_date,
        session_open_utc,
        session_close_utc,
        previous_trading_date: previous_trading_date.unwrap_or(market_date),
        is_early_close: truth.is_early_close,
        is_trading_day: truth.is_trading_day,
        calendar_source: truth.source,
        coverage_state,
    }
}

/// Select the same active calendar provider the autonomous session
/// controller uses (`session_controller::autonomous_session_schedule_from_env`'s
/// selection order): a configured [`FixedWindowOverrideProvider`]
/// (`MQK_SESSION_START_HH_MM`/`MQK_SESSION_STOP_HH_MM`) if both are set and
/// valid, else [`NyseWeekdaysProvider`]. Composition, not a second selection
/// policy — reuses `super::session_controller::session_window_from_env`.
pub fn active_calendar_provider_from_env() -> Box<dyn MarketCalendarProvider> {
    match super::session_controller::session_window_from_env() {
        Some(window) => Box::new(FixedWindowOverrideProvider { window }),
        None => Box::new(NyseWeekdaysProvider),
    }
}
