//! SMART-CALENDAR-SESSION-PROVIDER-01: Market calendar and session provider seam.
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
//! # Fail-closed contract (SCSP07)
//!
//! `MarketSessionTruth::is_in_session()` returns `false` for every state except
//! `RegularOpen`.  `Unknown` is explicitly fail-closed: providers must return it
//! when truth cannot be determined, and callers must treat it as session-closed.

use chrono::{DateTime, Utc};
use mqk_integrity::{nyse_is_early_close_today, CalendarSpec};

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
/// holiday + early-close table for 2023–2026.
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
