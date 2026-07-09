//! ASSET-CORE-05I — per-instrument session-routing parity proof.
//!
//! Proves `route_instrument_session_for_metadata` (`ASSET-CORE-05H`,
//! `mqk-daemon/src/state/market_calendar.rs`) truthfully routes every
//! canonical `InstrumentRegistryV2` asset class to its documented session
//! profile, that non-equity profiles are always reported model-only and
//! never as matching production, that the newly-explicit `"rate"` arm is
//! distinguished from a truly unrecognized asset-class string, and that
//! equity/ETF routing matches real production `NyseWeekdaysProvider` truth
//! at fixed timestamps already proven by
//! `scenario_market_calendar_session_provider_01.rs` /
//! `scenario_instrument_session_parity_status_shadow_asset_core_05c.rs`.
//!
//! Pure-function tests only: no DB pool, no provider/broker call, no
//! daemon/HTTP route, no runtime start, no order submission, no config or
//! env-var mutation. `route_instrument_session_for_metadata` itself is not
//! called by any route or production path — see its module docs.

use chrono::{DateTime, Utc};
use mqk_daemon::state::{
    self, InstrumentSessionRoute, MarketCalendarProvider, MarketSessionProfile,
    NyseWeekdaysProvider, SessionProfileResolutionTruth,
};
use mqk_md::instrument_registry_v2::CANONICAL_ASSET_CLASSES_V2;

// Known fixed timestamps, reused verbatim from
// `scenario_instrument_session_parity_status_shadow_asset_core_05c.rs` (which
// in turn cites `scenario_market_calendar_session_provider_01.rs` as the
// proof source) so this file does not re-derive calendar truth independently:
// - 2026-06-25T15:00:00Z -> RegularOpen (a Thursday)
// - 2026-07-03T14:00:00Z -> Holiday (observed Independence Day)
// - 2024-11-29T17:59:00Z -> RegularOpen, is_early_close=true (before early close)
// - 2024-11-29T18:01:00Z -> EarlyClose (after early close)
// - 2026-06-27T15:00:00Z -> RegularOpen (a Saturday would be Closed; this is
//   used only as "some other weekday" for crypto/forex continuity checks)
const TS_REGULAR_OPEN: &str = "2026-06-25T15:00:00Z";
const TS_HOLIDAY: &str = "2026-07-03T14:00:00Z";
const TS_BEFORE_EARLY_CLOSE: &str = "2024-11-29T17:59:00Z";
const TS_AFTER_EARLY_CLOSE: &str = "2024-11-29T18:01:00Z";
const TS_SATURDAY_CLOSED: &str = "2026-06-27T15:00:00Z";

fn ts(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .unwrap_or_else(|e| panic!("fixed test timestamp {raw} must parse: {e}"))
        .with_timezone(&Utc)
}

fn production_equity_state_str(now_utc: DateTime<Utc>) -> &'static str {
    NyseWeekdaysProvider.session_for(now_utc).state.as_str()
}

// ── Equity/ETF: production-backed, must match real NyseWeekdaysProvider ────

#[test]
fn ac05i_01_equity_routes_to_equity_profile_and_matches_production_at_regular_open() {
    let now = ts(TS_REGULAR_OPEN);
    let route = state::route_instrument_session_for_metadata("equity", None, now);

    assert_eq!(
        route.profile_resolution.profile,
        Some(MarketSessionProfile::EquityUsRegular)
    );
    assert_eq!(
        route.profile_resolution.truth_state,
        SessionProfileResolutionTruth::Active
    );
    assert!(route.is_production_backed());
    assert!(!route.is_model_only());
    assert_eq!(route.session_state, production_equity_state_str(now));
    assert_eq!(route.session_state, "regular_open");
}

#[test]
fn ac05i_02_etf_routes_to_equity_profile_same_as_plain_equity() {
    let now = ts(TS_REGULAR_OPEN);
    let route = state::route_instrument_session_for_metadata("equity", Some("etf"), now);

    assert_eq!(
        route.profile_resolution.profile,
        Some(MarketSessionProfile::EquityUsRegular)
    );
    assert!(route.is_production_backed());
    assert_eq!(route.session_state, production_equity_state_str(now));
    assert!(route.profile_resolution.reason.contains("etf"));
}

#[test]
fn ac05i_03_equity_matches_production_across_holiday_and_early_close_timestamps() {
    for raw in [TS_HOLIDAY, TS_BEFORE_EARLY_CLOSE, TS_AFTER_EARLY_CLOSE] {
        let now = ts(raw);
        let route = state::route_instrument_session_for_metadata("equity", None, now);
        assert_eq!(
            route.session_state,
            production_equity_state_str(now),
            "equity route must match production NyseWeekdaysProvider truth at {raw}"
        );
        assert!(
            route.is_production_backed(),
            "must stay production-backed at {raw}"
        );
    }
    // Sanity: these three timestamps are not all the same state, proving the
    // parity check above is exercising real variation, not a coincidence.
    let holiday = state::route_instrument_session_for_metadata("equity", None, ts(TS_HOLIDAY));
    let early_close =
        state::route_instrument_session_for_metadata("equity", None, ts(TS_AFTER_EARLY_CLOSE));
    assert_ne!(holiday.session_state, early_close.session_state);
}

// ── Non-equity: always model-only, never reported as production-matching ───

#[test]
fn ac05i_04_crypto_routes_to_crypto_continuous_and_is_never_production_backed() {
    for raw in [TS_REGULAR_OPEN, TS_HOLIDAY, TS_SATURDAY_CLOSED] {
        let route = state::route_instrument_session_for_metadata("crypto", None, ts(raw));
        assert_eq!(
            route.profile_resolution.profile,
            Some(MarketSessionProfile::CryptoContinuous)
        );
        assert_eq!(
            route.profile_resolution.truth_state,
            SessionProfileResolutionTruth::UnsupportedAssetClass
        );
        assert!(route.is_model_only());
        assert!(!route.is_production_backed());
        assert_eq!(route.session_state, "continuous");
    }
}

#[test]
fn ac05i_05_future_and_futures_alias_both_route_to_futures_globex_model_only() {
    for alias in ["future", "futures"] {
        let route = state::route_instrument_session_for_metadata(alias, None, ts(TS_REGULAR_OPEN));
        assert_eq!(
            route.profile_resolution.profile,
            Some(MarketSessionProfile::FuturesGlobex)
        );
        assert!(route.is_model_only());
        assert!(!route.is_production_backed());
    }
}

#[test]
fn ac05i_06_forex_routes_to_forex_24x5_and_is_model_only() {
    let weekday = state::route_instrument_session_for_metadata("forex", None, ts(TS_REGULAR_OPEN));
    assert_eq!(
        weekday.profile_resolution.profile,
        Some(MarketSessionProfile::ForexWeekdayContinuous)
    );
    assert!(weekday.is_model_only());
    assert!(!weekday.is_production_backed());
    assert_eq!(weekday.session_state, "continuous");

    let saturday =
        state::route_instrument_session_for_metadata("forex", None, ts(TS_SATURDAY_CLOSED));
    assert_eq!(saturday.session_state, "closed");
    assert!(saturday.is_model_only());
    assert!(!saturday.is_production_backed());
}

// ── Option/rate: no modeled session-profile shape, but distinguishable ─────
// from a truly unrecognized asset_class (the ASSET-CORE-05H fix).

#[test]
fn ac05i_07_option_and_options_alias_have_no_session_profile_shape() {
    for alias in ["option", "options"] {
        let route = state::route_instrument_session_for_metadata(alias, None, ts(TS_REGULAR_OPEN));
        assert_eq!(route.profile_resolution.profile, None);
        assert_eq!(
            route.profile_resolution.truth_state,
            SessionProfileResolutionTruth::UnsupportedAssetClass
        );
        assert!(
            !route.is_model_only(),
            "profile=None must not report model_only"
        );
        assert!(!route.is_production_backed());
    }
}

#[test]
fn ac05i_08_rate_is_explicit_unsupported_not_conflated_with_unknown() {
    let route = state::route_instrument_session_for_metadata("rate", None, ts(TS_REGULAR_OPEN));

    assert_eq!(route.profile_resolution.profile, None);
    assert_eq!(
        route.profile_resolution.truth_state,
        SessionProfileResolutionTruth::UnsupportedAssetClass,
        "rate is a recognized, schema-valid asset class (CANONICAL_ASSET_CLASSES_V2); \
         it must never resolve to Unknown"
    );
    assert!(route.profile_resolution.reason.contains("rate"));
    assert!(!route.is_model_only());
    assert!(!route.is_production_backed());
}

#[test]
fn ac05i_09_unrecognized_asset_class_fails_closed_as_unknown_distinct_from_rate() {
    for garbage in ["stock", "bond", "warrant"] {
        let route =
            state::route_instrument_session_for_metadata(garbage, None, ts(TS_REGULAR_OPEN));
        assert_eq!(route.profile_resolution.profile, None);
        assert_eq!(
            route.profile_resolution.truth_state,
            SessionProfileResolutionTruth::Unknown
        );
        assert!(!route.is_model_only());
        assert!(!route.is_production_backed());
    }

    // Blank/whitespace-only also fails closed to Unknown.
    let blank = state::route_instrument_session_for_metadata("   ", None, ts(TS_REGULAR_OPEN));
    assert_eq!(
        blank.profile_resolution.truth_state,
        SessionProfileResolutionTruth::Unknown
    );

    // Contrast: "rate" (schema-valid) is NOT Unknown, proving the two
    // classes of "no session profile" are truthfully distinguished.
    let rate = state::route_instrument_session_for_metadata("rate", None, ts(TS_REGULAR_OPEN));
    assert_ne!(
        rate.profile_resolution.truth_state,
        SessionProfileResolutionTruth::Unknown
    );
}

// ── Full canonical-class coverage guard ─────────────────────────────────────

#[test]
fn ac05i_10_every_canonical_v2_asset_class_is_recognized_not_unknown() {
    // Regression guard: if a future patch adds a new asset class to
    // CANONICAL_ASSET_CLASSES_V2 without also teaching the session router
    // about it, this test fails loudly instead of the class silently falling
    // through to the generic "unrecognized asset_class" Unknown arm.
    for &asset_class in CANONICAL_ASSET_CLASSES_V2 {
        let route =
            state::route_instrument_session_for_metadata(asset_class, None, ts(TS_REGULAR_OPEN));
        assert_ne!(
            route.profile_resolution.truth_state,
            SessionProfileResolutionTruth::Unknown,
            "canonical v2 asset_class '{asset_class}' must not resolve to Unknown"
        );
    }
}

// ── No enablement concept: routing is a pure function of metadata + time ───

#[test]
fn ac05i_11_routing_is_deterministic_and_has_no_enablement_parameter() {
    // `route_instrument_session_for_metadata` takes no `enabled`/
    // `paper_trading_enabled`/`live_trading_enabled` parameter at all — this
    // is a structural (compile-time-shaped) proof that enablement cannot
    // influence session-profile routing. Determinism: identical inputs
    // always produce an identical, `Eq`-comparable result.
    let now = ts(TS_REGULAR_OPEN);
    let a: InstrumentSessionRoute =
        state::route_instrument_session_for_metadata("crypto", None, now);
    let b: InstrumentSessionRoute =
        state::route_instrument_session_for_metadata("crypto", None, now);
    assert_eq!(a, b);

    let c = state::route_instrument_session_for_metadata("equity", None, now);
    let d = state::route_instrument_session_for_metadata("equity", None, now);
    assert_eq!(c, d);
}
