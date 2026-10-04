//! Shared causal session mapping for the calendar engines (turn-of-month, Halloween).
//!
//! Every calendar decision is a pure function of the latest completed bar's session
//! identity and the `us_equity_regular_sessions_v1` authority in `mqk-integrity`. Nothing
//! here reads a price, a following price row, or a weekday heuristic; anything the calendar
//! cannot resolve is `None` and the engine fails closed to FLAT.

use chrono::NaiveDate;
use mqk_integrity::sessions;
use sha2::{Digest, Sha256};

use crate::{BarStub, SemanticIdentityBuilder};

/// `N(t)`: the first regular session strictly after the session represented by the latest
/// completed bar. `None` (caller stays flat) when there is no bar, the latest bar is
/// incomplete, its label is not a 00:00 ET daily label of a covered regular session, or the
/// next session lies outside the calendar coverage.
pub(crate) fn next_session_of_latest_bar(recent: &[BarStub]) -> Option<NaiveDate> {
    let latest = recent.last()?;
    if !latest.is_complete {
        return None;
    }
    let session = sessions::session_of_daily_bar(latest.end_ts).ok()?;
    sessions::next_session_after(session).ok()
}

/// sha256 (hex) of the calendar's canonical content string.
pub(crate) fn calendar_content_sha256() -> String {
    hex::encode(Sha256::digest(sessions::calendar_content_string().as_bytes()))
}

/// Binds the calendar contract id and its content hash into a fingerprint, so changing any
/// closure, the coverage or the contract changes the identity of every dependent trial.
pub(crate) fn push_calendar_identity(b: &mut SemanticIdentityBuilder) {
    b.push_str(&format!("calendar:{}", sessions::US_EQUITY_REGULAR_SESSIONS_V1))
        .push_str(&calendar_content_sha256());
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
        assert_eq!(next_session_of_latest_bar(&[bar(d(2024, 3, 28), 1, false)]), None);
        // Holiday / weekend label, non-midnight label, uncovered dates.
        assert_eq!(next_session_of_latest_bar(&[bar(d(2024, 3, 29), 1, true)]), None);
        assert_eq!(next_session_of_latest_bar(&[bar(d(2024, 3, 30), 1, true)]), None);
        let mut off = bar(d(2024, 3, 28), 1, true);
        off.end_ts += 3600;
        assert_eq!(next_session_of_latest_bar(&[off]), None);
        assert_eq!(next_session_of_latest_bar(&[bar(d(2015, 12, 31), 1, true)]), None);
        assert_eq!(next_session_of_latest_bar(&[bar(d(2026, 12, 31), 1, true)]), None);
        assert_eq!(next_session_of_latest_bar(&[bar(d(2027, 6, 1), 1, true)]), None);
    }
}
