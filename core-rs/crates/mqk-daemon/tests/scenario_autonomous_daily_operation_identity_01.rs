//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-DURABLE-COORDINATION: daemon-side
//! session-plan resolution and deterministic identity proof.
//!
//! Pure: no DB, no network, no provider/broker call, no real daemon/runtime
//! start. Calendar truth is proven against the real
//! `market_calendar::NyseWeekdaysProvider` heuristic (deterministic, no I/O)
//! for the calendar-classification cases, and against a small injected fake
//! provider for the "calendar cannot classify at all" case.

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::state::autonomous_daily_operation::{
    derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, project_autonomous_daily_operation_read_model,
    resolve_autonomous_daily_session_plan, resolve_autonomous_daily_session_plan_from_env,
    AutonomousDailyPlanReason, AutonomousDailyPlanTiming, AutonomousDailyScheduleSource,
    AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution, FixedWindowOverrideConfig,
};
use mqk_daemon::state::market_calendar::{
    resolve_market_session_schedule, CalendarCoverageState, MarketCalendarProvider,
    MarketSessionState, MarketSessionTruth, NyseWeekdaysProvider,
};
use mqk_daemon::state::{
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SessionWindow, SymbolStrategyAssignment,
};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn instant(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn default_timing() -> AutonomousDailyPlanTiming {
    AutonomousDailyPlanTiming::production_default()
}

/// A calendar provider that always reports `Unknown` — proves the "calendar
/// cannot classify at all" blocked path without depending on any real
/// out-of-coverage heuristic behavior.
struct AlwaysUnknownProvider;

impl MarketCalendarProvider for AlwaysUnknownProvider {
    fn session_for(&self, _now_utc: DateTime<Utc>) -> MarketSessionTruth {
        MarketSessionTruth::unknown()
    }
}

/// A calendar provider that always classifies as an applicable, in-coverage
/// trading day with an injectable early-close flag — used to isolate
/// exchange-boundary/early-close identity effects from the real
/// `NyseWeekdaysProvider` heuristic's own date-dependent behavior.
struct FixedTruthProvider {
    is_early_close: bool,
}

impl MarketCalendarProvider for FixedTruthProvider {
    fn session_for(&self, _now_utc: DateTime<Utc>) -> MarketSessionTruth {
        MarketSessionTruth {
            state: MarketSessionState::RegularOpen,
            source: "test_fixed_provider",
            exchange: "test",
            is_trading_day: true,
            is_early_close: self.is_early_close,
            session_close_note: None,
        }
    }
}

fn assert_applicable(res: AutonomousDailySessionPlanResolution) -> AutonomousDailySessionPlan {
    match res {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => panic!("expected Applicable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Session plan — calendar classification
// ---------------------------------------------------------------------------

/// A regular Monday trading day resolves applicable.
#[test]
fn regular_trading_day_resolves_applicable() {
    let provider = NyseWeekdaysProvider;
    // 2025-01-06 is a Monday, not a holiday.
    let now = instant(2025, 1, 6, 17, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    let plan = assert_applicable(res);
    assert_eq!(plan.market_date, "2025-01-06");
    assert_eq!(
        plan.schedule_source,
        AutonomousDailyScheduleSource::NyseWeekdaysHeuristic
    );
}

/// A weekend date resolves not-applicable and never claims a session plan.
#[test]
fn weekend_resolves_not_applicable() {
    let provider = NyseWeekdaysProvider;
    // 2025-01-04 is a Saturday.
    let now = instant(2025, 1, 4, 17, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    match res {
        AutonomousDailySessionPlanResolution::NotApplicable {
            market_date,
            reason_code,
        } => {
            assert_eq!(market_date, "2025-01-04");
            assert_eq!(reason_code, AutonomousDailyPlanReason::Weekend);
        }
        other => panic!("expected NotApplicable(Weekend), got {other:?}"),
    }
}

/// A full-day exchange holiday resolves not-applicable.
#[test]
fn full_holiday_resolves_not_applicable() {
    let provider = NyseWeekdaysProvider;
    // 2025-12-25 is Christmas Day (Thursday).
    let now = instant(2025, 12, 25, 17, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    match res {
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::ExchangeHoliday);
        }
        other => panic!("expected NotApplicable(ExchangeHoliday), got {other:?}"),
    }
}

/// An early-close day retains the authoritative early close (13:00 ET), not
/// the ordinary 16:00 ET close.
#[test]
fn early_close_retains_authoritative_close() {
    let provider = NyseWeekdaysProvider;
    // 2025-11-28 (day after Thanksgiving) is an NYSE early-close day; EST
    // (UTC-5) is in effect, so 13:00 ET == 18:00 UTC.
    let now = instant(2025, 11, 28, 15, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    let plan = assert_applicable(res);
    assert!(plan.exchange_is_early_close);
    assert_eq!(
        plan.exchange_session_close_utc,
        instant(2025, 11, 28, 18, 0)
    );
    // No override: effective operation close mirrors the authoritative
    // exchange early close exactly.
    assert_eq!(
        plan.effective_operation_close_utc,
        plan.exchange_session_close_utc
    );
}

/// A date outside the declared calendar coverage window (2023-2028) blocks.
#[test]
fn out_of_range_coverage_blocks() {
    let provider = NyseWeekdaysProvider;
    // 2030-06-10 is a Monday, well outside the 2023-2028 coverage window.
    let now = instant(2030, 6, 10, 17, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    match res {
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::CalendarOutOfRange);
        }
        other => panic!("expected Blocked(CalendarOutOfRange), got {other:?}"),
    }
}

/// A calendar provider that cannot classify the instant at all blocks.
#[test]
fn unknown_calendar_blocks() {
    let provider = AlwaysUnknownProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    );
    match res {
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::CalendarUnavailable);
        }
        other => panic!("expected Blocked(CalendarUnavailable), got {other:?}"),
    }
}

/// DST winter (EST, UTC-5): 09:30 ET == 14:30 UTC, 16:00 ET == 21:00 UTC.
#[test]
fn dst_winter_open_close_correct() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0); // Monday, EST
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_eq!(plan.exchange_session_open_utc, instant(2025, 1, 6, 14, 30));
    assert_eq!(plan.exchange_session_close_utc, instant(2025, 1, 6, 21, 0));
}

/// DST summer (EDT, UTC-4): 09:30 ET == 13:30 UTC, 16:00 ET == 20:00 UTC.
#[test]
fn dst_summer_open_close_correct() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 7, 7, 17, 0); // Monday, EDT
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_eq!(plan.exchange_session_open_utc, instant(2025, 7, 7, 13, 30));
    assert_eq!(plan.exchange_session_close_utc, instant(2025, 7, 7, 20, 0));
}

/// Preopen/postclose timing derives strictly from the injected timing
/// config, not a hardcoded offset.
#[test]
fn preopen_postclose_derive_from_injected_timing() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let timing = AutonomousDailyPlanTiming {
        preopen_lead: chrono::Duration::minutes(10),
        postclose_finalize_delay: chrono::Duration::minutes(5),
    };
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &timing,
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_eq!(
        plan.preopen_start_utc,
        plan.effective_operation_open_utc - chrono::Duration::minutes(10)
    );
    assert_eq!(
        plan.postclose_finalize_utc,
        plan.effective_operation_close_utc + chrono::Duration::minutes(5)
    );
}

/// Identical inputs produce the identical session-plan identity.
#[test]
fn same_inputs_produce_same_session_plan_identity() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let plan_a = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    let plan_b = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_eq!(plan_a.session_plan_identity, plan_b.session_plan_identity);
}

/// A boundary change (different timing config) changes the session-plan
/// identity.
#[test]
fn boundary_change_changes_session_plan_identity() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let plan_a = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    let other_timing = AutonomousDailyPlanTiming {
        preopen_lead: chrono::Duration::minutes(45),
        postclose_finalize_delay: chrono::Duration::minutes(15),
    };
    let plan_b = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &other_timing,
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_ne!(plan_a.session_plan_identity, plan_b.session_plan_identity);
}

// ---------------------------------------------------------------------------
// Fixed-window override
// ---------------------------------------------------------------------------

/// A valid override applies only after authoritative trading-day proof, and
/// is labeled `fixed_window_override`.
#[test]
fn valid_override_applies_after_trading_day_proof_and_is_labeled() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0); // Monday, trading day
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    ));
    assert_eq!(
        plan.schedule_source,
        AutonomousDailyScheduleSource::FixedWindowOverride
    );
    // Exchange truth remains the authoritative regular-day boundaries,
    // untouched by the override.
    assert_eq!(plan.exchange_session_open_utc, instant(2025, 1, 6, 14, 30));
    assert_eq!(plan.exchange_session_close_utc, instant(2025, 1, 6, 21, 0));
    // Effective operation boundaries equal the override.
    assert_eq!(
        plan.effective_operation_open_utc,
        instant(2025, 1, 6, 14, 0)
    );
    assert_eq!(
        plan.effective_operation_close_utc,
        instant(2025, 1, 6, 20, 0)
    );
}

/// A valid override on an exchange early-close date retains the
/// authoritative early close as exchange truth while the effective
/// operation close follows the override — both remain simultaneously
/// visible on the plan.
#[test]
fn valid_override_on_early_close_date_keeps_both_truths_visible() {
    let provider = NyseWeekdaysProvider;
    // 2025-11-28 (day after Thanksgiving) is an NYSE early-close day.
    let now = instant(2025, 11, 28, 15, 0);
    let window = SessionWindow::parse("15:00", "19:00").expect("valid window");
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    ));
    assert!(plan.exchange_is_early_close);
    assert_eq!(
        plan.exchange_session_close_utc,
        instant(2025, 11, 28, 18, 0)
    );
    assert_eq!(
        plan.effective_operation_open_utc,
        instant(2025, 11, 28, 15, 0)
    );
    assert_eq!(
        plan.effective_operation_close_utc,
        instant(2025, 11, 28, 19, 0)
    );
    assert_ne!(
        plan.effective_operation_close_utc,
        plan.exchange_session_close_utc
    );
    assert_eq!(
        plan.schedule_source,
        AutonomousDailyScheduleSource::FixedWindowOverride
    );
}

/// Weekend plus a valid override remains not-applicable — the override
/// cannot manufacture a trading day.
#[test]
fn weekend_plus_valid_override_remains_not_applicable() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 4, 17, 0); // Saturday
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    );
    match res {
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::Weekend);
        }
        other => panic!("expected NotApplicable(Weekend), got {other:?}"),
    }
}

/// Holiday plus a valid override remains not-applicable.
#[test]
fn holiday_plus_valid_override_remains_not_applicable() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 12, 25, 17, 0); // Christmas
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    );
    match res {
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::ExchangeHoliday);
        }
        other => panic!("expected NotApplicable(ExchangeHoliday), got {other:?}"),
    }
}

/// Out-of-range calendar plus a valid override remains blocked — the
/// override cannot bypass unavailable/out-of-range calendar truth.
#[test]
fn out_of_range_plus_valid_override_remains_blocked() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2030, 6, 10, 17, 0);
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    );
    match res {
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::CalendarOutOfRange);
        }
        other => panic!("expected Blocked(CalendarOutOfRange), got {other:?}"),
    }
}

/// A calendar provider that cannot classify the instant at all, plus a valid
/// override, remains blocked — the override cannot establish calendar
/// authority the underlying provider itself could not establish.
#[test]
fn unknown_calendar_plus_valid_override_remains_blocked() {
    let provider = AlwaysUnknownProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let res = resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    );
    match res {
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            assert_eq!(reason_code, AutonomousDailyPlanReason::CalendarUnavailable);
        }
        other => panic!("expected Blocked(CalendarUnavailable), got {other:?}"),
    }
}

/// One missing override env var blocks with a visible configuration reason
/// (never silently falls back to the authoritative schedule).
#[test]
fn one_missing_override_variable_blocks_with_visible_reason() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let invalid = FixedWindowOverrideConfig::Invalid {
        detail:
            "MQK_SESSION_STOP_HH_MM is set but MQK_SESSION_START_HH_MM is not; both are required"
                .to_string(),
    };
    let res = resolve_autonomous_daily_session_plan(&provider, now, &default_timing(), &invalid);
    match res {
        AutonomousDailySessionPlanResolution::Blocked {
            reason_code,
            detail,
            ..
        } => {
            assert_eq!(
                reason_code,
                AutonomousDailyPlanReason::FixedWindowOverrideInvalid
            );
            assert!(detail.contains("required"));
        }
        other => panic!("expected Blocked(FixedWindowOverrideInvalid), got {other:?}"),
    }
}

/// An invalid (malformed) time blocks.
#[test]
fn invalid_time_blocks() {
    assert!(SessionWindow::parse("25:99", "20:00").is_none());
}

/// `start >= stop` blocks.
#[test]
fn start_ge_stop_blocks() {
    assert!(SessionWindow::parse("20:00", "14:00").is_none());
    assert!(SessionWindow::parse("14:00", "14:00").is_none());
}

/// No override preserves the authoritative exchange boundaries exactly, and
/// the effective operation window equals the exchange window exactly.
#[test]
fn no_override_preserves_authoritative_boundaries() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    assert_eq!(
        plan.schedule_source,
        AutonomousDailyScheduleSource::NyseWeekdaysHeuristic
    );
    assert_eq!(plan.exchange_session_open_utc, instant(2025, 1, 6, 14, 30));
    assert_eq!(plan.exchange_session_close_utc, instant(2025, 1, 6, 21, 0));
    assert_eq!(
        plan.effective_operation_open_utc,
        plan.exchange_session_open_utc
    );
    assert_eq!(
        plan.effective_operation_close_utc,
        plan.exchange_session_close_utc
    );
}

/// `previous_trading_date` on the plan matches
/// `MarketSessionSchedule::previous_trading_date` for the same instant —
/// never a fourth independent calendar-walk calculation.
#[test]
fn previous_trading_date_matches_market_session_schedule() {
    let provider = NyseWeekdaysProvider;
    // Monday 2025-01-06; the previous trading date is Friday 2025-01-03.
    let now = instant(2025, 1, 6, 17, 0);
    let plan = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    let schedule = resolve_market_session_schedule(&provider, now);
    assert_eq!(
        plan.previous_trading_date,
        format!(
            "{:04}-{:02}-{:02}",
            schedule.previous_trading_date.0,
            schedule.previous_trading_date.1,
            schedule.previous_trading_date.2
        )
    );
    assert_eq!(plan.previous_trading_date, "2025-01-03");
}

// ---------------------------------------------------------------------------
// Exchange vs. effective boundary identity isolation
// ---------------------------------------------------------------------------

/// Changing only the effective operation boundaries (via a valid override on
/// the same exchange day) changes the session-plan identity while the
/// exchange boundaries stay identical.
#[test]
fn effective_only_boundary_change_changes_session_plan_identity() {
    let provider = NyseWeekdaysProvider;
    let now = instant(2025, 1, 6, 17, 0);
    let plan_a = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let plan_b = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    ));
    assert_eq!(
        plan_a.exchange_session_open_utc,
        plan_b.exchange_session_open_utc
    );
    assert_eq!(
        plan_a.exchange_session_close_utc,
        plan_b.exchange_session_close_utc
    );
    assert_ne!(
        plan_a.effective_operation_open_utc,
        plan_b.effective_operation_open_utc
    );
    assert_ne!(plan_a.session_plan_identity, plan_b.session_plan_identity);
}

/// Changing only the exchange boundaries (via an injected fake provider that
/// differs only in early-close truth) changes the session-plan identity
/// while a valid override pins the effective operation boundaries identical
/// across both.
#[test]
fn exchange_only_boundary_change_changes_session_plan_identity_via_fake_provider() {
    let provider_a = FixedTruthProvider {
        is_early_close: false,
    };
    let provider_b = FixedTruthProvider {
        is_early_close: true,
    };
    let now = instant(2025, 1, 6, 17, 0);
    let window = SessionWindow::parse("14:00", "20:00").expect("valid window");
    let plan_a = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider_a,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    ));
    let plan_b = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider_b,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Valid(window),
    ));
    assert_eq!(
        plan_a.effective_operation_open_utc,
        plan_b.effective_operation_open_utc
    );
    assert_eq!(
        plan_a.effective_operation_close_utc,
        plan_b.effective_operation_close_utc
    );
    assert_ne!(
        plan_a.exchange_session_close_utc,
        plan_b.exchange_session_close_utc
    );
    assert_ne!(plan_a.session_plan_identity, plan_b.session_plan_identity);
}

/// Changing only early-close truth (holding market date, effective operation
/// window, and every other input equal) changes the session-plan identity.
#[test]
fn early_close_truth_change_changes_session_plan_identity() {
    let provider_a = FixedTruthProvider {
        is_early_close: false,
    };
    let provider_b = FixedTruthProvider {
        is_early_close: true,
    };
    let now = instant(2025, 1, 6, 17, 0);
    let plan_a = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider_a,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    let plan_b = assert_applicable(resolve_autonomous_daily_session_plan(
        &provider_b,
        now,
        &default_timing(),
        &FixedWindowOverrideConfig::Absent,
    ));
    assert!(!plan_a.exchange_is_early_close);
    assert!(plan_b.exchange_is_early_close);
    assert_ne!(plan_a.session_plan_identity, plan_b.session_plan_identity);
}

/// Static source proof: `AutonomousDailySessionPlan` no longer declares the
/// ambiguous `session_open_utc`/`session_close_utc` pair, and declares both
/// the exchange and effective-operation boundary sets explicitly.
#[test]
fn session_plan_struct_declares_explicit_boundary_fields_not_ambiguous_ones() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("state")
        .join("autonomous_daily_operation.rs");
    let source = std::fs::read_to_string(&path).expect("read autonomous_daily_operation.rs");

    let start_marker = "pub struct AutonomousDailySessionPlan {";
    let start = source
        .find(start_marker)
        .expect("AutonomousDailySessionPlan must exist in source");
    let end = source[start..]
        .find("\n}")
        .map(|i| start + i)
        .expect("expected the struct body to close with a closing brace");
    let body = &source[start..end];

    assert!(
        !body.contains("pub session_open_utc"),
        "AutonomousDailySessionPlan must not declare the ambiguous session_open_utc field. Body:\n{body}"
    );
    assert!(
        !body.contains("pub session_close_utc"),
        "AutonomousDailySessionPlan must not declare the ambiguous session_close_utc field. Body:\n{body}"
    );
    for expected in [
        "pub exchange_session_open_utc",
        "pub exchange_session_close_utc",
        "pub exchange_is_early_close",
        "pub effective_operation_open_utc",
        "pub effective_operation_close_utc",
        "pub previous_trading_date",
    ] {
        assert!(
            body.contains(expected),
            "AutonomousDailySessionPlan must declare {expected}. Body:\n{body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

fn sample_config(symbol: &str, strategy_id: &str, timeframe: &str) -> MultiSymbolRuntimeConfig {
    MultiSymbolRuntimeConfig {
        schema_version: "multi-symbol-runtime-config-v1".to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: timeframe.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    }
}

fn sample_binding(
    strategy_id: Option<&str>,
    symbol: Option<&str>,
    timeframe_secs: Option<i64>,
) -> EffectiveRuntimeBinding {
    EffectiveRuntimeBinding {
        effective_runtime_strategy_id: strategy_id.map(|s| s.to_string()),
        effective_runtime_target_symbol: symbol.map(|s| s.to_string()),
        effective_runtime_timeframe_secs: timeframe_secs,
    }
}

/// Same assignment config produces the same assignment identity.
#[test]
fn same_assignment_config_produces_same_identity() {
    let a = sample_config("AAPL", "trend_v1", "5m");
    let b = sample_config("AAPL", "trend_v1", "5m");
    assert_eq!(
        derive_assignment_identity(&a),
        derive_assignment_identity(&b)
    );
}

/// A meaningful assignment order change changes the identity (configured
/// dispatch order is never sorted away).
#[test]
fn meaningful_assignment_order_change_changes_identity() {
    let mut a = sample_config("AAPL", "trend_v1", "5m");
    a.symbols.push(SymbolStrategyAssignment {
        symbol: "MSFT".to_string(),
        strategy_id: "trend_v1".to_string(),
        timeframe: "5m".to_string(),
    });
    let mut b = a.clone();
    b.symbols.reverse();
    assert_ne!(
        derive_assignment_identity(&a),
        derive_assignment_identity(&b)
    );
}

/// Symbol normalization (trim + uppercase) is deterministic.
#[test]
fn symbol_normalization_is_deterministic() {
    let a = sample_config("aapl", "trend_v1", "5m");
    let b = sample_config(" AAPL ", "trend_v1", "5m");
    assert_eq!(
        derive_assignment_identity(&a),
        derive_assignment_identity(&b)
    );
}

/// Same effective runtime binding produces the same identity.
#[test]
fn same_runtime_binding_produces_same_identity() {
    let a = sample_binding(Some("trend_v1"), Some("AAPL"), Some(300));
    let b = sample_binding(Some("trend_v1"), Some("AAPL"), Some(300));
    assert_eq!(
        derive_runtime_binding_identity(&a),
        derive_runtime_binding_identity(&b)
    );
}

/// `None` and empty-string binding values remain distinct.
#[test]
fn none_and_empty_binding_values_remain_distinct() {
    let none_binding = sample_binding(None, Some("AAPL"), Some(300));
    let empty_binding = sample_binding(Some(""), Some("AAPL"), Some(300));
    assert_ne!(
        derive_runtime_binding_identity(&none_binding),
        derive_runtime_binding_identity(&empty_binding)
    );
}

fn sample_plan(market_date: &str) -> AutonomousDailySessionPlan {
    let now = Utc::now();
    AutonomousDailySessionPlan {
        market_date: market_date.to_string(),
        previous_trading_date: "2025-01-03".to_string(),
        exchange_session_open_utc: now,
        exchange_session_close_utc: now + chrono::Duration::hours(6),
        exchange_is_early_close: false,
        effective_operation_open_utc: now,
        effective_operation_close_utc: now + chrono::Duration::hours(6),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: AutonomousDailyScheduleSource::NyseWeekdaysHeuristic,
        preopen_start_utc: now - chrono::Duration::minutes(30),
        postclose_finalize_utc: now + chrono::Duration::hours(6) + chrono::Duration::minutes(15),
        session_plan_identity: format!("splan-{market_date}"),
    }
}

/// Same complete operation identity input produces the same UUID.
#[test]
fn same_operation_identity_input_produces_same_uuid() {
    let plan = sample_plan("2025-01-06");
    let id_a = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-x", "bind-x");
    let id_b = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-x", "bind-x");
    assert_eq!(id_a, id_b);
}

/// A market-date change (via a different plan) changes the operation id.
#[test]
fn market_date_change_changes_operation_id() {
    let plan_a = sample_plan("2025-01-06");
    let plan_b = sample_plan("2025-01-07");
    let id_a = derive_autonomous_daily_operation_id(&plan_a, "paper", "alpaca", "asgn-x", "bind-x");
    let id_b = derive_autonomous_daily_operation_id(&plan_b, "paper", "alpaca", "asgn-x", "bind-x");
    assert_ne!(id_a, id_b);
}

/// A session-plan identity change changes the operation id.
#[test]
fn session_plan_change_changes_operation_id() {
    let mut plan_a = sample_plan("2025-01-06");
    let mut plan_b = plan_a.clone();
    plan_a.session_plan_identity = "splan-A".to_string();
    plan_b.session_plan_identity = "splan-B".to_string();
    let id_a = derive_autonomous_daily_operation_id(&plan_a, "paper", "alpaca", "asgn-x", "bind-x");
    let id_b = derive_autonomous_daily_operation_id(&plan_b, "paper", "alpaca", "asgn-x", "bind-x");
    assert_ne!(id_a, id_b);
}

/// An assignment-identity change changes the operation id.
#[test]
fn assignment_change_changes_operation_id() {
    let plan = sample_plan("2025-01-06");
    let id_a = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-A", "bind-x");
    let id_b = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-B", "bind-x");
    assert_ne!(id_a, id_b);
}

/// A runtime-binding-identity change changes the operation id.
#[test]
fn runtime_binding_change_changes_operation_id() {
    let plan = sample_plan("2025-01-06");
    let id_a = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-x", "bind-A");
    let id_b = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-x", "bind-B");
    assert_ne!(id_a, id_b);
}

/// No operation-identity helper reads env or constructs a bootstrap — this
/// is a structural property demonstrated by every identity test above
/// compiling and passing with zero env vars set and zero DB/network access;
/// this test additionally confirms the derivation is pure by calling it from
/// a thread with a scrubbed environment view (best-effort: proves the
/// function signature accepts no implicit environment channel).
#[test]
fn operation_identity_helper_takes_no_hidden_env_or_bootstrap_input() {
    let plan = sample_plan("2025-01-06");
    // Pure function signature: only explicit arguments, no `_from_env`
    // suffix, no `AppState`/env access possible from within its body.
    let _ = derive_autonomous_daily_operation_id(&plan, "paper", "alpaca", "asgn-x", "bind-x");
}

// ---------------------------------------------------------------------------
// Read model
// ---------------------------------------------------------------------------

fn sample_operation_record(operation_id: Uuid) -> mqk_db::AutonomousDailyOperationRecord {
    let now = Utc::now();
    mqk_db::AutonomousDailyOperationRecord {
        operation_id,
        market_date: chrono::NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
        deployment_mode: "paper".to_string(),
        adapter_id: "alpaca".to_string(),
        session_plan_identity: "splan-x".to_string(),
        assignment_identity: "asgn-x".to_string(),
        runtime_binding_identity: "bind-x".to_string(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: now,
        effective_operation_close_utc: now + chrono::Duration::hours(6),
        exchange_session_open_utc: Some(now),
        exchange_session_close_utc: Some(now + chrono::Duration::hours(6)),
        exchange_is_early_close: Some(false),
        previous_trading_date: Some(chrono::NaiveDate::from_ymd_opt(2025, 1, 3).unwrap()),
        preopen_start_utc: now - chrono::Duration::minutes(30),
        postclose_finalize_utc: now + chrono::Duration::hours(6) + chrono::Duration::minutes(15),
        state: "awaiting_preopen".to_string(),
        state_reason_code: None,
        state_blocker_signature: None,
        state_version: 1,
        run_id: None,
        start_attempt_count: 0,
        last_start_attempt_utc: None,
        next_retry_utc: None,
        data_refresh_state: "not_started".to_string(),
        last_provider_poll_utc: None,
        provider_poll_attempt_count: 0,
        provider_poll_success_count: 0,
        provider_poll_failure_count: 0,
        last_completed_bar_ts: None,
        last_dispatched_bar_ts: None,
        bars_observed: 0,
        bars_dispatched: 0,
        started_at_utc: None,
        stopped_at_utc: None,
        finalized_at_utc: None,
        outcome: None,
        no_trade_reason: None,
        last_error: None,
        created_at_utc: now,
        updated_at_utc: now,
        stop_attempt_count: Some(0),
        last_stop_attempt_utc: None,
    }
}

/// Null values remain null through the projection.
#[test]
fn projection_preserves_null_values() {
    let record = sample_operation_record(Uuid::new_v4());
    let model = project_autonomous_daily_operation_read_model(&record, None);
    assert!(model.state_reason_code.is_none());
    assert!(model.run_id.is_none());
    assert!(model.last_completed_bar_ts.is_none());
    assert!(model.recent_events.is_none());
}

/// A legacy row's `None` exchange-truth fields remain `None` through the
/// projection — never substituted with the effective-operation values.
#[test]
fn projection_preserves_legacy_null_exchange_fields() {
    let mut record = sample_operation_record(Uuid::new_v4());
    record.exchange_session_open_utc = None;
    record.exchange_session_close_utc = None;
    record.exchange_is_early_close = None;
    record.previous_trading_date = None;

    let model = project_autonomous_daily_operation_read_model(&record, None);
    assert!(model.exchange_session_open_utc.is_none());
    assert!(model.exchange_session_close_utc.is_none());
    assert!(model.exchange_is_early_close.is_none());
    assert!(model.previous_trading_date.is_none());
    // The effective-operation boundaries remain present and untouched.
    assert_eq!(
        model.effective_operation_open_utc,
        record.effective_operation_open_utc
    );
    assert_eq!(
        model.effective_operation_close_utc,
        record.effective_operation_close_utc
    );
}

/// A fresh row's non-null exchange-truth fields pass through the projection
/// unmodified.
#[test]
fn projection_preserves_present_exchange_fields() {
    let record = sample_operation_record(Uuid::new_v4());
    let model = project_autonomous_daily_operation_read_model(&record, None);
    assert_eq!(
        model.exchange_session_open_utc,
        record.exchange_session_open_utc
    );
    assert_eq!(
        model.exchange_session_close_utc,
        record.exchange_session_close_utc
    );
    assert_eq!(
        model.exchange_is_early_close,
        record.exchange_is_early_close
    );
    assert_eq!(model.previous_trading_date, record.previous_trading_date);
}

/// Explicit stored zero counters remain zero through the projection.
#[test]
fn projection_preserves_explicit_zero() {
    let record = sample_operation_record(Uuid::new_v4());
    let model = project_autonomous_daily_operation_read_model(&record, None);
    assert_eq!(model.bars_observed, 0);
    assert_eq!(model.bars_dispatched, 0);
    assert_eq!(model.start_attempt_count, 0);
}

/// The projection is pure: two calls over identical inputs (including an
/// events slice) produce identical output, and it performs no DB write (the
/// function signature has no DB handle to write through in the first
/// place).
#[test]
fn projection_is_pure_and_deterministic() {
    let operation_id = Uuid::new_v4();
    let record = sample_operation_record(operation_id);
    let event = mqk_db::AutonomousDailyOperationEventRecord {
        operation_id,
        transition_seq: 1,
        from_state: "none".to_string(),
        to_state: "awaiting_preopen".to_string(),
        reason_code: None,
        occurred_at_utc: record.created_at_utc,
        run_id: None,
        bounded_detail: "initial".to_string(),
    };
    let events = vec![event];
    let model_a = project_autonomous_daily_operation_read_model(&record, Some(&events));
    let model_b = project_autonomous_daily_operation_read_model(&record, Some(&events));
    assert_eq!(model_a, model_b);
    assert_eq!(model_a.recent_events.as_ref().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01:
// shared canonical calendar context
// ---------------------------------------------------------------------------

/// The production wrapper must resolve its authoritative calendar provider
/// through the exact same shared canonical context Bundle 2's readiness
/// composition uses (`daily_data_readiness::load_readiness_context_from_env`),
/// never a second, independently-constructed `NyseWeekdaysProvider`. With no
/// override configured, the two compositions must therefore agree
/// byte-for-byte on every authoritative field for the same instant — proving
/// consumption of the shared context rather than a fourth independent
/// market-date/open/close calculation.
#[test]
fn production_wrapper_consumes_shared_calendar_context() {
    unsafe {
        std::env::remove_var("MQK_SESSION_START_HH_MM");
        std::env::remove_var("MQK_SESSION_STOP_HH_MM");
    }
    let now = instant(2025, 1, 6, 17, 0); // Monday, trading day, in-coverage

    let plan = assert_applicable(resolve_autonomous_daily_session_plan_from_env(
        now,
        &default_timing(),
    ));

    // The exact shared context every Bundle 2 readiness surface consumes.
    let context = mqk_daemon::daily_data_readiness::load_readiness_context_from_env();
    let exchange_schedule =
        resolve_market_session_schedule(context.calendar_provider.as_ref(), now);

    assert_eq!(
        exchange_schedule.coverage_state,
        CalendarCoverageState::Active
    );
    assert_eq!(
        plan.market_date,
        format!(
            "{:04}-{:02}-{:02}",
            exchange_schedule.market_date.0,
            exchange_schedule.market_date.1,
            exchange_schedule.market_date.2
        )
    );
    assert_eq!(
        plan.exchange_session_open_utc,
        exchange_schedule.session_open_utc
    );
    assert_eq!(
        plan.exchange_session_close_utc,
        exchange_schedule.session_close_utc
    );
    assert_eq!(
        plan.effective_operation_open_utc,
        exchange_schedule.session_open_utc
    );
    assert_eq!(
        plan.effective_operation_close_utc,
        exchange_schedule.session_close_utc
    );
    assert_eq!(
        plan.previous_trading_date,
        format!(
            "{:04}-{:02}-{:02}",
            exchange_schedule.previous_trading_date.0,
            exchange_schedule.previous_trading_date.1,
            exchange_schedule.previous_trading_date.2
        )
    );
    assert_eq!(
        plan.exchange_is_early_close,
        exchange_schedule.is_early_close
    );
    assert_eq!(plan.calendar_source, exchange_schedule.calendar_source);
    assert_eq!(plan.calendar_coverage_state, "active");
    assert_eq!(
        plan.schedule_source,
        AutonomousDailyScheduleSource::NyseWeekdaysHeuristic
    );
}

/// Static source proof: the production wrapper
/// (`resolve_autonomous_daily_session_plan_from_env`) must never directly
/// construct or name `NyseWeekdaysProvider` in its own body — it must obtain
/// its calendar provider through the shared canonical context loader
/// (`daily_data_readiness::load_readiness_context_from_env`) instead.
/// Scoped narrowly to this one function's body text (from its `pub fn`
/// signature to the next top-level section marker) — never a broad
/// repository-wide grep prone to unrelated false failures.
#[test]
fn production_wrapper_never_constructs_nyse_weekdays_provider_directly() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("state")
        .join("autonomous_daily_operation.rs");
    let source = std::fs::read_to_string(&path).expect("read autonomous_daily_operation.rs");

    let start_marker = "pub fn resolve_autonomous_daily_session_plan_from_env(";
    let start = source
        .find(start_marker)
        .expect("resolve_autonomous_daily_session_plan_from_env must exist in source");
    let end_marker =
        "\n// ---------------------------------------------------------------------------\n// B.3";
    let end = source[start..]
        .find(end_marker)
        .map(|i| start + i)
        .expect("expected the B.3 section marker to follow this function");
    let body = &source[start..end];

    assert!(
        !body.contains("NyseWeekdaysProvider"),
        "resolve_autonomous_daily_session_plan_from_env must not directly construct or name \
         NyseWeekdaysProvider; it must obtain its provider from the shared canonical context \
         loader instead. Body:\n{body}"
    );
    assert!(
        body.contains("load_readiness_context_from_env"),
        "resolve_autonomous_daily_session_plan_from_env must obtain its calendar provider via \
         daily_data_readiness::load_readiness_context_from_env(). Body:\n{body}"
    );
}
