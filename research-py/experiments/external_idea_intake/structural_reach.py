"""Calendar-only analysis of the proposed calendar hypotheses. No price, return or provider is read.

Four kinds of statement are kept apart:

1. EXACT CALENDAR EXPOSURE: which sessions a rule holds, from the pinned session calendar
   (us_equity_regular_sessions_v1) over the native evaluation window.
2. ESTABLISHED PROMOTION CONSTRAINT (profitable months): the evaluator buckets the candidate's own continuous
   equity curve by UTC month and counts a month as profitable only if month-end equity is strictly above the previous
   month-end (`w[1] > w[0]`). The backtest equity is cash plus mark-to-market with no interest or carry, so a month in
   which the candidate holds no position and makes no fill cannot be profitable. The months in which equity can change
   are the months of the held sessions plus the month of the fill that follows each run (next-bar execution). That gives
   a valid UPPER BOUND on the profitable-months fraction, and, for a rule whose active months recur sparsely, a span of
   months beyond which the declared minimum cannot be reached on any contiguous equity curve.
3. HYPOTHETICAL ILLUSTRATION (not a requirement): alpha is `candidate_total_return_pct - benchmark_account_return_pct`.
   The benchmark holds the candidate's FIRST resolved entry quantity from the candidate's first long-decision bar to the
   end; later candidate entries are re-sized, and both runs pay the candidate's costs. Whether alpha is non-negative is
   a comparison of two realized profit-and-loss paths and depends on prices. Only if every session had the same price
   change, quantities were equal and costs were zero would a rule held a fraction e of sessions have to earn 1/e times
   the passive per-session amount; that dilution multiple is reported as an illustration of scale, nothing more.
4. GENUINE ECONOMIC OUTCOMES (alpha, Sharpe, drawdown, DSR, PBO, profit factor): require price data and are not computed.
"""

from __future__ import annotations

import datetime as dt
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "alpha_edge_census_01"))
import calendar_authority as ca  # noqa: E402

EVAL_START = dt.date(2016, 3, 1)           # native fold window: ten 12-month folds from 2016-03-01
EVAL_END = dt.date(2026, 2, 28)            # last day before the Final Holdout start (2026-03-01)
AD_HOC_CLOSURES = frozenset({dt.date(2018, 12, 5), dt.date(2025, 1, 9)})   # unannounced national days of mourning
MIN_PROFITABLE_MONTHS_PCT = 0.40           # promotion_policy value carried unchanged in every frozen declaration
_COVERAGE_MONTHS = [(y, m) for y in range(2016, 2027) for m in range(1, 13)]


def eval_sessions() -> list[dt.date]:
    return ca.sessions_between(EVAL_START, EVAL_END)


def _preceding(sessions_all: list[dt.date], day: dt.date, n: int) -> list[dt.date]:
    before = [s for s in sessions_all if s < day]
    return before[-n:]


def pre_holiday(*, scheduled_only: bool) -> set[dt.date]:
    allsess = ca.sessions_between(dt.date(2016, 1, 1), dt.date(2026, 12, 31))
    out: set[dt.date] = set()
    for c in ca.CLOSURES:
        if c.weekday() >= 5 or (scheduled_only and c in AD_HOC_CLOSURES):
            continue
        out.update(_preceding(allsess, c, 2))
    return out


def santa_claus() -> set[dt.date]:
    out: set[dt.date] = set()
    for y in range(2016, 2027):
        dec = ca.sessions_between(dt.date(y, 12, 1), dt.date(y, 12, 31))
        jan = ca.sessions_between(dt.date(y + 1, 1, 1), dt.date(y + 1, 1, 31)) if y < 2026 else []
        out.update(dec[-5:])
        out.update(jan[:2])
    return out


def opex_week() -> set[dt.date]:
    out: set[dt.date] = set()
    for y in range(2016, 2027):
        for m in range(1, 13):
            fridays = [dt.date(y, m, d) for d in range(1, 29) if dt.date(y, m, d).weekday() == 4]
            expiry = fridays[2]          # a closure on the expiry Friday simply drops out of the session list
            monday = expiry - dt.timedelta(days=expiry.weekday())
            out.update(s for s in ca.sessions_between(monday, expiry))
    return out


def pnl_months(members: set[dt.date]) -> set[tuple[int, int]]:
    """Months in which equity can change: the held sessions' months plus the month of the next-bar fill that closes
    each run (the first session after the last held session of a run)."""
    allsess = ca.sessions_between(dt.date(2016, 1, 1), dt.date(2026, 12, 31))
    months = {(s.year, s.month) for s in members}
    for i, s in enumerate(allsess[:-1]):
        if s in members and allsess[i + 1] not in members:
            nxt = allsess[i + 1]
            months.add((nxt.year, nxt.month))
    return months


def reach(members: set[dt.date]) -> dict:
    sess = eval_sessions()
    exposed = [s for s in sess if s in members]
    months = sorted({(s.year, s.month) for s in sess})
    held_months = {(s.year, s.month) for s in exposed}
    active = {m for m in pnl_months(members) if m in set(months)}
    transitions = len(months) - 1
    # the first month has no previous month-end to compare against, so it never contributes a transition
    bound = len(active - {months[0]})
    return {
        # 1. exact calendar exposure
        "eval_sessions": len(sess), "exposed_sessions": len(exposed),
        "exposure_fraction": len(exposed) / len(sess),
        "months": len(months), "months_with_exposure": len(held_months),
        "months_with_possible_equity_change": len(active),
        # 2. upper bound implied by the strict month-over-month rule
        "max_profitable_months_fraction": bound / transitions,
        "profitable_months_gate_reachable": bound / transitions >= MIN_PROFITABLE_MONTHS_PCT,
        # 3. hypothetical illustration only (uniform per-session price change, equal quantity, zero cost)
        "hypothetical_uniform_dilution_multiple": len(sess) / len(exposed) if exposed else None,
    }


def max_reachable_span_months(members: set[dt.date], min_pct: float = MIN_PROFITABLE_MONTHS_PCT) -> int:
    """Longest run of consecutive calendar months, anywhere in 2016-2026, over which the minimum profitable-months
    fraction is still reachable. Any single continuous equity curve covering more months than this cannot reach it,
    whatever the prices, because a month without a possible equity change never counts."""
    active = pnl_months(members)
    best = 0
    for s in range(len(_COVERAGE_MONTHS)):
        for length in range(2, len(_COVERAGE_MONTHS) - s + 1):
            span = _COVERAGE_MONTHS[s:s + length]
            changing = sum(1 for m in span[1:] if m in active)
            if changing / (length - 1) >= min_pct:
                best = max(best, length)
    return best


def summary() -> dict:
    return {
        "EXT-032 pre-holiday, scheduled holidays only": reach(pre_holiday(scheduled_only=True)),
        "EXT-032 pre-holiday, every full closure": reach(pre_holiday(scheduled_only=False)),
        "EXT-169 Santa Claus": reach(santa_claus()),
        "EXT-044 opex week (monthly third Friday)": reach(opex_week()),
    }


if __name__ == "__main__":
    for k, v in summary().items():
        print(k, {a: (round(b, 4) if isinstance(b, float) else b) for a, b in v.items()})
    print("EXT-169 longest reachable span (months):", max_reachable_span_months(santa_claus()))
