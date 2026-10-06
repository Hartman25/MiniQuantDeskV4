"""Corrected census time-partition fence. Discovery is strictly before DISCOVERY_END_EXCLUSIVE (2024-01-01).
2024 was read by the rejected execution and is CONTAMINATED_BY_REJECTED_RUN, never an unread reserve. The
remaining confirmation reserve and the final holdout are never read for economics."""

from __future__ import annotations

import pandas as pd

DATA_REQUEST_START_UTC = pd.Timestamp("2016-01-01", tz="UTC")
DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2024-01-01", tz="UTC")
CONTAMINATED_START = DISCOVERY_END_EXCLUSIVE
CONTAMINATED_END_EXCLUSIVE = pd.Timestamp("2025-01-01", tz="UTC")
RESERVE_START = CONTAMINATED_END_EXCLUSIVE
RESERVE_END_EXCLUSIVE = pd.Timestamp("2026-03-01", tz="UTC")
FINAL_HOLDOUT_START = RESERVE_END_EXCLUSIVE

DISCOVERY = "DISCOVERY"
CONTAMINATED = "CONTAMINATED_BY_REJECTED_RUN"
RESERVE = "REMAINING_CONFIRMATION_RESERVE"
HOLDOUT = "FINAL_HOLDOUT"

PARTITIONS = {
    "schema_version": "alpha_census_partitions_v2",
    "discovery": {"start_inclusive": "2016-01-01", "end_exclusive": "2024-01-01",
                  "role": "PASS1_CENSUS_DISCOVERY"},
    "contaminated_by_rejected_run": {"start_inclusive": "2024-01-01", "end_exclusive": "2025-01-01",
                                     "status": "CONTAMINATED_BY_REJECTED_RUN",
                                     "reason": "read by ALPHA_EDGE_CENSUS_01_REJECTED_EXECUTION_20261005",
                                     "role": "NEVER_AN_UNREAD_RESERVE"},
    "remaining_confirmation_reserve": {"start_inclusive": "2025-01-01", "end_exclusive": "2026-03-01",
                                       "status": "RESERVED_UNCONSUMED",
                                       "role": "LATER_INDEPENDENT_CONFIRMATION"},
    "final_holdout": {"start_inclusive": "2026-03-01", "status": "RESERVED_UNCONSUMED"},
}


class PartitionBreach(RuntimeError):
    pass


def classify_timestamp(ts) -> str:
    t = pd.Timestamp(ts)
    t = t.tz_localize("UTC") if t.tzinfo is None else t.tz_convert("UTC")
    if t < DISCOVERY_END_EXCLUSIVE:
        return DISCOVERY
    if t < CONTAMINATED_END_EXCLUSIVE:
        return CONTAMINATED
    if t < RESERVE_END_EXCLUSIVE:
        return RESERVE
    return HOLDOUT


def require_discovery_only(end_ts, *, what: str) -> None:
    """Fail closed if any timestamp reaches the contaminated year, the confirmation reserve or the final
    holdout. An empty input is also refused: a guard that checked nothing proves nothing."""
    ts = pd.to_datetime(pd.Series(end_ts), utc=True)
    if ts.empty:
        raise PartitionBreach(f"{what}: empty timestamp set proves nothing")
    latest = ts.max()
    if latest >= DISCOVERY_END_EXCLUSIVE:
        raise PartitionBreach(f"{what}: row at {latest.isoformat()} ({classify_timestamp(latest)}) is not strictly "
                              f"before {DISCOVERY_END_EXCLUSIVE.date()} (discovery fence)")
