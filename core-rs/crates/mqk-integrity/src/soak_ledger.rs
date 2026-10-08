//! M1.10 finite autonomous-Paper validation ledger (pure, no IO).
//!
//! Derives "N countable sessions" and "K consecutive clean sessions" from per-session
//! evidence records so that the count is never a manual increment. A record is evidence of
//! one finalized autonomous Paper daily operation; the ledger decides what counts:
//!
//! * the date must be a regular US-equity session per [`crate::calendar`], the Paper-runtime
//!   table (2023-2028; weekends, holidays and dates outside that coverage never count; there is
//!   no weekday fallback). The research `sessions` table ends earlier and is not the Paper
//!   authority;
//! * the session must have run under exactly the accepted post-repair code SHA (a session under
//!   any other SHA never counts, so a correctness repair restarts the count);
//! * the record must carry exactly the policy's [`DeploymentIdentity`] (strategy, symbol,
//!   timeframe, runtime domain), so evidence of one deployment never counts for another;
//! * that deployed identity must have held `active_paper` authority;
//! * the finalized outcome must be a completed one (activity or an evidenced no-trade day);
//! * duplicate records of one date collapse when identical (retry/restart idempotency) and
//!   exclude the date when they conflict.
//!
//! A countable session with any invalidator is *dirty*: it counts toward the total but ends the
//! clean run. "Consecutive" is over consecutive regular sessions: a regular session with no
//! countable record between two clean sessions breaks the run.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::calendar;

/// Finalized durable outcome of the day's autonomous operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    CompletedWithActivity,
    CompletedNoTrade,
    /// Never started, evidence blocked/degraded, manual intervention, or unfinalized.
    NotCompleted,
}

/// The canonical deployed identity a session is evidence for: the promotion identity
/// `(strategy_id, symbol, timeframe_secs)` plus the runtime domain. Callers supply canonical
/// (already normalized) values; the ledger compares exactly and never normalizes or defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentIdentity {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub runtime_domain: String,
}

impl DeploymentIdentity {
    fn is_complete(&self) -> bool {
        !self.strategy_id.is_empty()
            && !self.symbol.is_empty()
            && self.timeframe_secs > 0
            && !self.runtime_domain.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub market_date: NaiveDate,
    /// The deployment this session is evidence for.
    pub deployment: DeploymentIdentity,
    /// Git SHA the daemon ran under for this session.
    pub code_sha: String,
    /// The exact deployed binding held `active_paper` authority for the whole session.
    pub active_paper_promotion: bool,
    pub outcome: SessionOutcome,
    /// Runbook invalidators observed during the session (empty = clean).
    pub invalidators: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerPolicy {
    /// The accepted final post-repair SHA; sessions under any other SHA never count.
    pub accepted_code_sha: String,
    /// The one deployment whose sessions count; an incomplete identity counts nothing.
    pub deployment: DeploymentIdentity,
    pub required_sessions: u32,
    pub required_consecutive_clean: u32,
}

impl LedgerPolicy {
    /// The frozen M1.10 gate: 10 countable sessions and 5 consecutive clean sessions.
    pub fn m1_10(accepted_code_sha: impl Into<String>, deployment: DeploymentIdentity) -> Self {
        Self {
            accepted_code_sha: accepted_code_sha.into(),
            deployment,
            required_sessions: 10,
            required_consecutive_clean: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    NotARegularSession,
    OutOfCoverage,
    WrongCodeSha,
    WrongDeployment,
    NoActivePaperPromotion,
    NotCompleted,
    ConflictingDuplicate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerVerdict {
    pub countable_sessions: u32,
    pub longest_clean_run: u32,
    pub trailing_clean_run: u32,
    pub exclusions: Vec<(NaiveDate, Exclusion)>,
    pub passed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Clean,
    Dirty,
    Excluded,
}

type Normalized = (
    DeploymentIdentity,
    String,
    bool,
    SessionOutcome,
    Vec<String>,
);

fn normalized(r: &SessionRecord) -> Normalized {
    let mut inv = r.invalidators.clone();
    inv.sort();
    (
        r.deployment.clone(),
        r.code_sha.clone(),
        r.active_paper_promotion,
        r.outcome,
        inv,
    )
}

fn classify(policy: &LedgerPolicy, records: &[&SessionRecord]) -> Result<bool, Exclusion> {
    let first = normalized(records[0]);
    if records.iter().any(|r| normalized(r) != first) {
        return Err(Exclusion::ConflictingDuplicate);
    }
    let r = records[0];
    match calendar::nyse_is_regular_session_date(r.market_date) {
        Err(_) => return Err(Exclusion::OutOfCoverage),
        Ok(false) => return Err(Exclusion::NotARegularSession),
        Ok(true) => {}
    }
    if policy.accepted_code_sha.is_empty() || r.code_sha != policy.accepted_code_sha {
        return Err(Exclusion::WrongCodeSha);
    }
    if !policy.deployment.is_complete() || r.deployment != policy.deployment {
        return Err(Exclusion::WrongDeployment);
    }
    if !r.active_paper_promotion {
        return Err(Exclusion::NoActivePaperPromotion);
    }
    if r.outcome == SessionOutcome::NotCompleted {
        return Err(Exclusion::NotCompleted);
    }
    Ok(r.invalidators.is_empty())
}

/// Evaluate the ledger. Order-independent and idempotent over duplicate records.
pub fn evaluate(policy: &LedgerPolicy, records: &[SessionRecord]) -> LedgerVerdict {
    let mut by_date: BTreeMap<NaiveDate, Vec<&SessionRecord>> = BTreeMap::new();
    for r in records {
        by_date.entry(r.market_date).or_default().push(r);
    }

    let mut standing: BTreeMap<NaiveDate, Standing> = BTreeMap::new();
    let mut exclusions = Vec::new();
    for (date, recs) in &by_date {
        match classify(policy, recs) {
            Ok(true) => standing.insert(*date, Standing::Clean),
            Ok(false) => standing.insert(*date, Standing::Dirty),
            Err(e) => {
                exclusions.push((*date, e));
                standing.insert(*date, Standing::Excluded)
            }
        };
    }
    let countable_sessions = standing
        .values()
        .filter(|s| **s != Standing::Excluded)
        .count() as u32;

    // Walk consecutive regular sessions from the first to the last recorded regular session.
    let regular: Vec<NaiveDate> = by_date
        .keys()
        .copied()
        .filter(|d| calendar::nyse_is_regular_session_date(*d) == Ok(true))
        .collect();
    let (mut longest, mut run) = (0u32, 0u32);
    if let (Some(first), Some(last)) = (regular.first().copied(), regular.last().copied()) {
        let mut cursor = first;
        loop {
            match standing.get(&cursor) {
                Some(Standing::Clean) => {
                    run += 1;
                    longest = longest.max(run);
                }
                _ => run = 0,
            }
            if cursor >= last {
                break;
            }
            match calendar::nyse_next_regular_session_after(cursor) {
                Ok(next) => cursor = next,
                Err(_) => break,
            }
        }
    }

    let passed = !policy.accepted_code_sha.is_empty()
        && countable_sessions >= policy.required_sessions
        && longest >= policy.required_consecutive_clean;
    LedgerVerdict {
        countable_sessions,
        longest_clean_run: longest,
        trailing_clean_run: run,
        exclusions,
        passed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "accepted-final-sha";

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn deployment() -> DeploymentIdentity {
        DeploymentIdentity {
            strategy_id: "h1_turn_of_month".into(),
            symbol: "SPY".into(),
            timeframe_secs: 86_400,
            runtime_domain: "paper".into(),
        }
    }

    fn rec(date: NaiveDate) -> SessionRecord {
        SessionRecord {
            market_date: date,
            deployment: deployment(),
            code_sha: SHA.to_string(),
            active_paper_promotion: true,
            outcome: SessionOutcome::CompletedNoTrade,
            invalidators: vec![],
        }
    }

    /// `n` consecutive regular sessions starting at (and including) the session `start`.
    fn run_of(start: NaiveDate, n: usize) -> Vec<NaiveDate> {
        let mut out = vec![start];
        while out.len() < n {
            out.push(calendar::nyse_next_regular_session_after(*out.last().unwrap()).unwrap());
        }
        out
    }

    fn policy() -> LedgerPolicy {
        LedgerPolicy::m1_10(SHA, deployment())
    }

    fn eval(records: &[SessionRecord]) -> LedgerVerdict {
        evaluate(&policy(), records)
    }

    fn clean_run(n: usize) -> Vec<SessionRecord> {
        run_of(d(2026, 10, 5), n).into_iter().map(rec).collect()
    }

    #[test]
    fn ten_clean_consecutive_sessions_pass_and_nine_do_not() {
        let v = eval(&clean_run(10));
        assert_eq!((v.countable_sessions, v.longest_clean_run), (10, 10));
        assert!(v.passed);
        let v = eval(&clean_run(9));
        assert_eq!(v.countable_sessions, 9);
        assert!(!v.passed, "one session short of 10");
    }

    #[test]
    fn weekends_and_holidays_never_increment() {
        let mut r = clean_run(9);
        r.push(rec(d(2026, 10, 10))); // Saturday
        r.push(rec(d(2026, 10, 4))); // Sunday
        r.push(rec(d(2026, 11, 26))); // Thanksgiving
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 9);
        assert!(!v.passed);
        for date in [d(2026, 10, 10), d(2026, 10, 4), d(2026, 11, 26)] {
            assert!(v
                .exclusions
                .contains(&(date, Exclusion::NotARegularSession)));
        }
    }

    #[test]
    fn early_close_day_is_a_countable_session() {
        let v = eval(&[rec(d(2026, 11, 27)), rec(d(2026, 12, 24))]);
        assert_eq!(v.countable_sessions, 2);
    }

    #[test]
    fn dates_outside_calendar_coverage_never_count() {
        let v = eval(&[rec(d(2029, 1, 2)), rec(d(2022, 12, 30))]);
        assert_eq!(v.countable_sessions, 0);
        assert!(v
            .exclusions
            .iter()
            .all(|(_, e)| *e == Exclusion::OutOfCoverage));
    }

    #[test]
    fn sessions_after_the_research_calendar_horizon_count_through_the_paper_runtime_calendar() {
        // 2026-12-30 .. 2027-01-15: the Paper runtime operates across the year boundary
        // (2026-12-31 and 2027-01-04.. are regular sessions; 2027-01-01 and 2027-01-18 are not).
        let sessions_2027 = [
            d(2026, 12, 30),
            d(2026, 12, 31),
            d(2027, 1, 4),
            d(2027, 1, 5),
            d(2027, 1, 6),
            d(2027, 1, 7),
            d(2027, 1, 8),
            d(2027, 1, 11),
            d(2027, 1, 12),
            d(2027, 1, 13),
        ];
        let mut r: Vec<SessionRecord> = sessions_2027.into_iter().map(rec).collect();
        r.push(rec(d(2027, 1, 1))); // New Year's Day: closed
        r.push(rec(d(2027, 1, 18))); // MLK Day: closed
        let v = eval(&r);
        assert_eq!((v.countable_sessions, v.longest_clean_run), (10, 10));
        assert!(v.passed);
        for date in [d(2027, 1, 1), d(2027, 1, 18)] {
            assert!(v
                .exclusions
                .contains(&(date, Exclusion::NotARegularSession)));
        }
    }

    #[test]
    fn the_same_session_twice_counts_once_and_retries_are_idempotent() {
        let mut r = clean_run(10);
        r.extend(clean_run(10)); // restart / replay of the same evidence
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 10);
        assert_eq!(v.longest_clean_run, 10);
        assert!(v.passed);
    }

    #[test]
    fn conflicting_duplicates_exclude_the_date_and_break_the_run() {
        let mut r = clean_run(10);
        let mut other = r[4].clone();
        other.outcome = SessionOutcome::CompletedWithActivity;
        r.push(other);
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 9);
        assert!(v
            .exclusions
            .contains(&(r[4].market_date, Exclusion::ConflictingDuplicate)));
        assert_eq!(v.longest_clean_run, 5, "the excluded date splits 4 + 5");
    }

    #[test]
    fn a_dirty_session_counts_but_resets_the_consecutive_clean_run() {
        let mut r = clean_run(10);
        r[4].invalidators = vec!["reconcile_dirty".into()];
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 10);
        assert_eq!((v.longest_clean_run, v.trailing_clean_run), (5, 5));
        assert!(v.passed);
        // Dirty in the middle of only nine clean ones -> no run of five.
        let mut r = clean_run(10);
        r[2].invalidators = vec!["x".into()];
        r[7].invalidators = vec!["y".into()];
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 10);
        assert_eq!(v.longest_clean_run, 4);
        assert!(!v.passed, "10 countable but no 5 consecutive clean");
    }

    #[test]
    fn an_unrecorded_regular_session_breaks_consecutiveness() {
        let mut r = clean_run(11);
        r.remove(5); // a regular session with no record
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 10);
        assert_eq!(v.longest_clean_run, 5);
        assert_eq!(v.trailing_clean_run, 5);
        r.remove(8); // a second unrecorded session splits the tail run
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 9);
        assert_eq!((v.longest_clean_run, v.trailing_clean_run), (5, 1));
        assert!(!v.passed, "9 countable sessions");
    }

    #[test]
    fn a_weekend_between_sessions_does_not_break_consecutiveness() {
        // Fri 2026-10-09 and Mon 2026-10-12 are consecutive regular sessions.
        let v = eval(&[rec(d(2026, 10, 9)), rec(d(2026, 10, 12))]);
        assert_eq!(v.longest_clean_run, 2);
    }

    #[test]
    fn sessions_under_any_other_sha_never_count_so_a_repair_restarts_the_count() {
        let mut r = clean_run(10);
        for x in r.iter_mut().take(6) {
            x.code_sha = "pre-repair-sha".into();
        }
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 4);
        assert!(!v.passed);
        assert_eq!(
            v.exclusions
                .iter()
                .filter(|(_, e)| *e == Exclusion::WrongCodeSha)
                .count(),
            6
        );
        // An empty accepted SHA fails closed.
        assert!(!evaluate(&LedgerPolicy::m1_10("", deployment()), &clean_run(10)).passed);
    }

    type Mutate = fn(&mut DeploymentIdentity);

    #[test]
    fn evidence_of_a_different_deployment_never_counts() {
        let cases: [(&str, Mutate); 4] = [
            ("strategy", |d| d.strategy_id = "other_strategy".into()),
            ("symbol", |d| d.symbol = "QQQ".into()),
            ("timeframe", |d| d.timeframe_secs = 3_600),
            ("runtime", |d| d.runtime_domain = "live".into()),
        ];
        for (label, mutate) in cases {
            let mut r = clean_run(10);
            mutate(&mut r[3].deployment);
            let v = eval(&r);
            assert_eq!(v.countable_sessions, 9, "{label}");
            assert!(!v.passed, "{label}");
            assert!(
                v.exclusions
                    .contains(&(r[3].market_date, Exclusion::WrongDeployment)),
                "{label}"
            );
            assert_eq!(
                v.longest_clean_run, 6,
                "{label}: the excluded date splits 3 + 6"
            );
            // The same record counts under a policy for that deployment.
            let mut p = policy();
            mutate(&mut p.deployment);
            let only = evaluate(&p, &r[3..4]);
            assert_eq!(only.countable_sessions, 1, "{label}");
        }
    }

    #[test]
    fn a_same_date_duplicate_with_a_conflicting_deployment_excludes_the_date() {
        let mut r = clean_run(10);
        let mut other = r[4].clone();
        other.deployment.symbol = "QQQ".into();
        r.push(other);
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 9);
        assert!(v
            .exclusions
            .contains(&(r[4].market_date, Exclusion::ConflictingDuplicate)));
    }

    #[test]
    fn an_incomplete_policy_deployment_counts_nothing() {
        let cases: [(&str, Mutate); 4] = [
            ("strategy", |d| d.strategy_id.clear()),
            ("symbol", |d| d.symbol.clear()),
            ("timeframe", |d| d.timeframe_secs = 0),
            ("runtime", |d| d.runtime_domain.clear()),
        ];
        for (label, mutate) in cases {
            let mut p = policy();
            mutate(&mut p.deployment);
            let r: Vec<SessionRecord> = clean_run(10)
                .into_iter()
                .map(|mut x| {
                    mutate(&mut x.deployment);
                    x
                })
                .collect();
            let v = evaluate(&p, &r);
            assert_eq!(v.countable_sessions, 0, "{label}");
            assert!(!v.passed, "{label}");
        }
    }

    #[test]
    fn no_active_paper_promotion_or_unfinished_outcome_never_counts() {
        let mut r = clean_run(10);
        r[0].active_paper_promotion = false;
        r[1].outcome = SessionOutcome::NotCompleted;
        let v = eval(&r);
        assert_eq!(v.countable_sessions, 8);
        assert!(v
            .exclusions
            .contains(&(r[0].market_date, Exclusion::NoActivePaperPromotion)));
        assert!(v
            .exclusions
            .contains(&(r[1].market_date, Exclusion::NotCompleted)));
    }

    #[test]
    fn the_verdict_is_order_independent() {
        let mut r = clean_run(10);
        r[3].invalidators = vec!["a".into(), "b".into()];
        let forward = eval(&r);
        r.reverse();
        assert_eq!(eval(&r), forward);
    }
}
