//! Shared causal session mapping for the calendar engines (turn-of-month, Halloween, monthly,
//! pre-holiday).
//!
//! Every calendar decision is a pure function of the latest completed bar's session
//! identity and a versioned session-calendar authority in `mqk-integrity`. Nothing here reads
//! a price, a following price row, or a weekday heuristic; anything the calendar cannot resolve
//! is `None` and the engine fails closed to FLAT.
//!
//! The authority is selected explicitly by a [`CalendarContract`]; an engine never switches
//! contract implicitly, and the contract id plus content hash are bound into its fingerprint.

use chrono::NaiveDate;
use mqk_integrity::{sessions, sessions_v2};
use sha2::{Digest, Sha256};

use crate::{BarStub, SemanticIdentityBuilder};

/// A versioned US-equity session-calendar contract. `V1` is frozen: every campaign registered
/// before the v2 contract keeps resolving through it, byte for byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarContract {
    V1,
    V2,
}

impl CalendarContract {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::V1 => sessions::US_EQUITY_REGULAR_SESSIONS_V1,
            Self::V2 => sessions_v2::US_EQUITY_REGULAR_SESSIONS_V2,
        }
    }

    pub(crate) fn content_string(self) -> String {
        match self {
            Self::V1 => sessions::calendar_content_string(),
            Self::V2 => sessions_v2::calendar_content_string(),
        }
    }

    /// sha256 (hex) of the contract's canonical content string.
    pub(crate) fn content_sha256(self) -> String {
        hex::encode(Sha256::digest(self.content_string().as_bytes()))
    }

    /// The covered regular session a daily-bar label represents; `None` on any refusal.
    pub(crate) fn session_of_daily_bar(self, end_ts: i64) -> Option<NaiveDate> {
        match self {
            Self::V1 => sessions::session_of_daily_bar(end_ts).ok(),
            Self::V2 => sessions_v2::session_of_daily_bar(end_ts).ok(),
        }
    }

    /// The first regular session strictly after `d`; `None` outside the coverage.
    pub(crate) fn next_session_after(self, d: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::V1 => sessions::next_session_after(d).ok(),
            Self::V2 => sessions_v2::next_session_after(d).ok(),
        }
    }
}

/// `N(t)`: the first regular session strictly after the session represented by the latest
/// completed bar. `None` (caller stays flat) when there is no bar, the latest bar is
/// incomplete, its label is not a 00:00 ET daily label of a covered regular session, or the
/// next session lies outside the calendar coverage.
pub(crate) fn next_session_of_latest_bar_in(
    contract: CalendarContract,
    recent: &[BarStub],
) -> Option<NaiveDate> {
    let latest = recent.last()?;
    if !latest.is_complete {
        return None;
    }
    let session = contract.session_of_daily_bar(latest.end_ts)?;
    contract.next_session_after(session)
}

/// [`next_session_of_latest_bar_in`] under the frozen v1 contract.
pub(crate) fn next_session_of_latest_bar(recent: &[BarStub]) -> Option<NaiveDate> {
    next_session_of_latest_bar_in(CalendarContract::V1, recent)
}

/// sha256 (hex) of the v1 calendar's canonical content string.
#[cfg(test)]
pub(crate) fn calendar_content_sha256() -> String {
    CalendarContract::V1.content_sha256()
}

/// Binds a contract id and its content hash into a fingerprint, so changing any closure, the
/// coverage or the contract changes the identity of every dependent trial.
pub(crate) fn push_calendar_identity_for(
    contract: CalendarContract,
    b: &mut SemanticIdentityBuilder,
) {
    b.push_str(&format!("calendar:{}", contract.id()))
        .push_str(&contract.content_sha256());
}

/// [`push_calendar_identity_for`] under the frozen v1 contract (identity unchanged).
pub(crate) fn push_calendar_identity(b: &mut SemanticIdentityBuilder) {
    push_calendar_identity_for(CalendarContract::V1, b);
}

#[cfg(test)]
pub(crate) mod test_support {
    use chrono::{NaiveDate, TimeZone};
    use chrono_tz::America::New_York;

    use crate::BarStub;

    pub fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    /// The provider's daily-bar label: 00:00:00 America/New_York of the session date.
    pub fn label(date: NaiveDate) -> i64 {
        New_York
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp()
    }

    pub fn bar(date: NaiveDate, close_micros: i64, complete: bool) -> BarStub {
        BarStub::new(label(date), complete, close_micros, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn calendar_content_hash_equals_the_predeclared_value() {
        assert_eq!(
            calendar_content_sha256(),
            "3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de"
        );
    }

    #[test]
    fn next_session_is_strictly_after_the_latest_completed_bar_session() {
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2024, 3, 28), 1, true)]),
            Some(d(2024, 4, 1))
        );
        // Only the LATEST bar matters; older bars cannot change the mapping.
        assert_eq!(
            next_session_of_latest_bar(&[
                bar(d(2020, 1, 2), 5, true),
                bar(d(2024, 3, 28), 9, true)
            ]),
            Some(d(2024, 4, 1))
        );
    }

    #[test]
    fn unresolvable_inputs_resolve_to_none() {
        assert_eq!(next_session_of_latest_bar(&[]), None);
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2024, 3, 28), 1, false)]),
            None
        );
        // Holiday / weekend label, non-midnight label, uncovered dates.
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2024, 3, 29), 1, true)]),
            None
        );
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2024, 3, 30), 1, true)]),
            None
        );
        let mut off = bar(d(2024, 3, 28), 1, true);
        off.end_ts += 3600;
        assert_eq!(next_session_of_latest_bar(&[off]), None);
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2015, 12, 31), 1, true)]),
            None
        );
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2026, 12, 31), 1, true)]),
            None
        );
        assert_eq!(
            next_session_of_latest_bar(&[bar(d(2027, 6, 1), 1, true)]),
            None
        );
    }

    #[test]
    fn contract_hashes_are_pinned_and_distinct() {
        assert_eq!(
            CalendarContract::V1.content_sha256(),
            "3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de"
        );
        assert_eq!(CalendarContract::V1.id(), "us_equity_regular_sessions_v1");
        assert_eq!(CalendarContract::V2.id(), "us_equity_regular_sessions_v2");
        assert_eq!(
            CalendarContract::V2.content_sha256(),
            "88b305dbe93f67f0cf7247e9a850234ce5b59ebe55bcec65fbee6842e8a49fe2"
        );
        assert_ne!(
            CalendarContract::V1.content_sha256(),
            CalendarContract::V2.content_sha256()
        );
    }

    #[test]
    fn v1_identity_push_is_byte_identical_to_the_historical_recipe() {
        use crate::semantic_identity::SEMANTIC_IDENTITY_SCHEMA_V1;
        let mut legacy = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, "x", "0.1.0");
        legacy
            .push_str("calendar:us_equity_regular_sessions_v1")
            .push_str("3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de");
        let mut via_seam = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, "x", "0.1.0");
        push_calendar_identity(&mut via_seam);
        assert_eq!(legacy.finish(), via_seam.finish());
        let mut v2 = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, "x", "0.1.0");
        push_calendar_identity_for(CalendarContract::V2, &mut v2);
        let mut v1 = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, "x", "0.1.0");
        push_calendar_identity(&mut v1);
        assert_ne!(v1.finish(), v2.finish());
    }

    #[test]
    fn v2_resolves_beyond_the_v1_horizon_and_v1_still_refuses_there() {
        let last_v1 = bar(d(2026, 12, 31), 1, true);
        assert_eq!(
            next_session_of_latest_bar_in(CalendarContract::V1, std::slice::from_ref(&last_v1)),
            None
        );
        assert_eq!(
            next_session_of_latest_bar_in(CalendarContract::V2, &[last_v1]),
            Some(d(2027, 1, 4))
        );
        assert_eq!(
            next_session_of_latest_bar_in(CalendarContract::V2, &[bar(d(2027, 6, 1), 1, true)]),
            Some(d(2027, 6, 2))
        );
        assert_eq!(
            next_session_of_latest_bar_in(CalendarContract::V2, &[bar(d(2028, 12, 29), 1, true)]),
            None,
            "v2 coverage ends 2028-12-31"
        );
        // A v1-covered date resolves identically under both contracts.
        for day in [
            d(2018, 12, 4),
            d(2024, 3, 28),
            d(2025, 1, 8),
            d(2026, 12, 30),
        ] {
            assert_eq!(
                next_session_of_latest_bar_in(CalendarContract::V1, &[bar(day, 1, true)]),
                next_session_of_latest_bar_in(CalendarContract::V2, &[bar(day, 1, true)]),
                "{day}"
            );
        }
    }
}
