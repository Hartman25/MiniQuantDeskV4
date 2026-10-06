"""Canonical US-equity regular-session calendar for calendar-effect cells (S14).

Closures are parsed from the Rust authority (mqk-integrity sessions.rs) and the content identity is pinned, so
calendar membership never comes from price rows."""

from __future__ import annotations

import datetime as dt
import hashlib
import re
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
SESSIONS_RS = REPO / "core-rs" / "crates" / "mqk-integrity" / "src" / "sessions.rs"
CONTRACT_ID = "us_equity_regular_sessions_v1"
COVERAGE_START = dt.date(2016, 1, 1)
COVERAGE_END = dt.date(2026, 12, 31)
EXPECTED_CONTENT_SHA256 = "3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de"


def _parse_closures(src: str) -> list[dt.date]:
    block = re.search(r"const FULL_CLOSURES:[^=]*=\s*&\[(.*?)\];", src, re.S)
    if block is None:
        raise RuntimeError("FULL_CLOSURES table not found in sessions.rs")
    return [dt.date(int(y), int(m), int(d)) for y, m, d in re.findall(r"\((\d{4}),\s*(\d+),\s*(\d+)\)", block.group(1))]


def content_string(closures: list[dt.date]) -> str:
    head = f"{CONTRACT_ID}\ncoverage={COVERAGE_START.isoformat()}..{COVERAGE_END.isoformat()}"
    return head + "".join(f"\nclosure={d.isoformat()}" for d in closures)


CLOSURES = _parse_closures(SESSIONS_RS.read_text(encoding="utf-8"))
CONTENT_SHA256 = hashlib.sha256(content_string(CLOSURES).encode("utf-8")).hexdigest()
if CONTENT_SHA256 != EXPECTED_CONTENT_SHA256 or CLOSURES != sorted(set(CLOSURES)):
    raise RuntimeError("session calendar content identity differs from the pinned authority; fail closed")
_CLOSED = frozenset(CLOSURES)


def is_session(d: dt.date) -> bool:
    if not (COVERAGE_START <= d <= COVERAGE_END):
        raise ValueError(f"{d} outside canonical session coverage")
    return d.weekday() < 5 and d not in _CLOSED


def sessions_between(start: dt.date, end_inclusive: dt.date) -> list[dt.date]:
    out, d = [], start
    while d <= end_inclusive:
        if is_session(d):
            out.append(d)
        d += dt.timedelta(days=1)
    return out


def month_session_positions(start: dt.date, end_inclusive: dt.date) -> dict[dt.date, tuple[int, int]]:
    """session date -> (1-based ordinal in month, sessions remaining in month incl. itself)."""
    by_month: dict[tuple[int, int], list[dt.date]] = {}
    for d in sessions_between(start, end_inclusive):
        by_month.setdefault((d.year, d.month), []).append(d)
    out: dict[dt.date, tuple[int, int]] = {}
    for days in by_month.values():
        for i, d in enumerate(days):
            out[d] = (i + 1, len(days) - i)
    return out
