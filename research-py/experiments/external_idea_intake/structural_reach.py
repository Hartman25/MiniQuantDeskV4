"""Calendar-only structural reach of the proposed calendar hypotheses against two accepted Promotion-side rules.

No price, return or provider is read: exposure is a pure function of the pinned session calendar
(us_equity_regular_sessions_v1) over the native evaluation window. The two rules examined:

* profitable-months gate (mqk-promotion evaluator; threshold = the declared MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT): a month counts as profitable only if month-end equity is strictly
  above the previous month-end equity, so a month with no exposure can never count;
* capital-matched buy-and-hold benchmark alpha: the benchmark holds the same quantity in every eligible session, so a
  candidate exposed for a fraction e of sessions must earn at least 1/e times the passive holder's average
  per-session return in its exposed sessions to reach alpha >= 0, whenever that passive return is positive.
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


def reach(members: set[dt.date]) -> dict:
    sess = eval_sessions()
    exposed = [s for s in sess if s in members]
    months = sorted({(s.year, s.month) for s in sess})
    exposed_months = {(s.year, s.month) for s in exposed}
    transitions = len(months) - 1
    # the first month has no previous month-end to compare against, so it never contributes a transition
    reachable_months = len(exposed_months - {months[0]})
    return {
        "eval_sessions": len(sess), "exposed_sessions": len(exposed),
        "exposure_fraction": len(exposed) / len(sess),
        "months": len(months), "months_with_exposure": len(exposed_months),
        "max_profitable_months_fraction": reachable_months / transitions,
        "profitable_months_gate_reachable": reachable_months / transitions >= MIN_PROFITABLE_MONTHS_PCT,
        "required_exposed_session_return_multiple_of_passive": len(sess) / len(exposed) if exposed else None,
    }


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
