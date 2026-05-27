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

use chrono::{DateTime, Utc};
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
