# M1.10 — Finite Autonomous Paper Validation (10 countable / 5 consecutive clean)

Scope: how the M1.10 exit gate is counted and evidenced. It does not authorize starting a soak,
arming Paper, or deploying a strategy. Live remains disabled.

## Gate (frozen)

At least **10 countable** autonomous Paper market sessions and at least **5 consecutive clean**
sessions, all after the final correctness repair.

## Counting authority

The count is derived, never incremented by hand: `mqk_integrity::soak_ledger::evaluate` over one
`SessionRecord` per finalized session, with `LedgerPolicy::m1_10(<accepted post-repair SHA>, <deployment identity>)`.
The deployment identity is `DeploymentIdentity { strategy_id, symbol, timeframe_secs, runtime_domain }` (the
promotion identity plus `paper`), supplied canonical by the caller and compared exactly; each record carries the
identity it is evidence for.
Verdict fields: `countable_sessions`, `longest_clean_run`, `trailing_clean_run`, `exclusions`,
`passed`. `passed` requires `countable_sessions >= 10` **and** `longest_clean_run >= 5`.

| A record counts only if | Otherwise excluded as |
|---|---|
| its date is a regular US-equity session per `us_equity_regular_sessions_v1` (covered through 2026-12-31) | `NotARegularSession` (weekend, holiday, Sunday) / `OutOfCoverage` (extend the calendar under a new identity first; never a weekday fallback) |
| it ran under exactly the accepted post-repair SHA | `WrongCodeSha` |
| its deployment identity equals the policy's (strategy, symbol, timeframe, runtime domain all equal; an incomplete policy identity counts nothing) | `WrongDeployment` |
| that deployed identity held `active_paper` for the whole session | `NoActivePaperPromotion` |
| the day's autonomous operation finalized as `completed_with_activity` or `completed_no_trade` | `NotCompleted` (never started, evidence blocked/degraded, manual intervention, unfinalized) |
| duplicate records of the date are identical, including deployment identity (restart/retry collapses to one) | `ConflictingDuplicate` (date excluded) |

A startup-only day, a day with no regular session, and any day before a valid promoted deployment
exists never count. A session with an invalidator is **dirty**: it counts toward 10 but ends the
clean run. A regular session with no countable record between two clean sessions also breaks the
run (a weekend or holiday does not).

## Invalidators (clean-run breakers)

Any of: unplanned halt or disarm; reconciliation dirty/stale/unavailable at session start or end;
`approved_for_live` observed true anywhere (stop-everything incident); a durable evidence write
failure or invalid dynamic-selection evidence (`autonomous_paper_ops.md` section 27); a selected-host
dispatch discrepancy; an unattended supervision gap; a fill or order for an identity without
`active_paper` authority.

## Repair boundary

Any correctness repair lands under a new accepted SHA. Sessions under the previous SHA stay
`WrongCodeSha` and the count restarts at zero. There is no carry-forward and no reasoned exception
in the ledger; record the new SHA before the first session that is meant to count.

## After each real session capture

1. `market_date`, the daemon's build SHA and the deployed `(strategy_id, symbol, timeframe_secs, runtime domain)`.
2. `GET /api/v1/strategy/promotions/check?strategy_id=...&symbol=...&timeframe_secs=...` before open
   and after close: `current_state` is `active_paper` both times.
3. The finalized `sys_autonomous_daily_operations` row: `state`, `outcome`, `no_trade_reason`,
   `start_attempt_count`, `run_id`.
4. Arm/halt/reconcile state at open and close, and every invalidator observed (or none).
5. The retained session evidence required by `autonomous_paper_ops.md` section 11/21/28.
6. One `SessionRecord` for the date; re-running the ledger over the full set gives the verdict.

## State

Not started. Blocked on a qualified candidate that has reached `active_paper` and a deployment of it
(M1.9). Weekends, holidays and the days before that deployment contribute nothing.

The Bundle 7 formal soak (`autonomous_paper_ops.md` Part 3, five supervised `paper_enforced`
sessions) is a separate, narrower contract; its manifests are evidence inputs, not the M1.10 count.
