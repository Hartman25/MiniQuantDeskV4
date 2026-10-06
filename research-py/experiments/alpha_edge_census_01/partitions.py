"""Pass-1 time-partition fence. Discovery is strictly before DISCOVERY_END_EXCLUSIVE; the confirmation
reserve and the final holdout are never read for economics."""

from __future__ import annotations

import pandas as pd

DISCOVERY_END_EXCLUSIVE = pd.Timestamp("2025-01-01", tz="UTC")
CONFIRMATION_RESERVE_START = pd.Timestamp("2025-01-01", tz="UTC")
CONFIRMATION_RESERVE_END_INCLUSIVE = pd.Timestamp("2026-02-27", tz="UTC")
FINAL_HOLDOUT_START = pd.Timestamp("2026-03-01", tz="UTC")
DATA_REQUEST_START_UTC = pd.Timestamp("2016-01-01", tz="UTC")

PARTITIONS = {
    "schema_version": "alpha_census_partitions_v1",
    "discovery": {"start_inclusive": "2016-01-01", "end_exclusive": "2025-01-01",
                  "role": "PASS1_CENSUS_DISCOVERY"},
    "confirmation_reserve": {"start_inclusive": "2025-01-01", "end_inclusive": "2026-02-27",
                             "status": "RESERVED_UNREAD", "role": "LATER_INDEPENDENT_CONFIRMATION"},
    "final_holdout": {"start_inclusive": "2026-03-01", "status": "RESERVED_UNCONSUMED"},
}


class PartitionBreach(RuntimeError):
    pass


def require_discovery_only(end_ts, *, what: str) -> None:
    """Fail closed if any timestamp reaches the confirmation reserve / final holdout. An empty input is
    also refused: a guard that checked nothing proves nothing."""
    ts = pd.to_datetime(pd.Series(end_ts), utc=True)
    if ts.empty:
        raise PartitionBreach(f"{what}: empty timestamp set proves nothing")
    latest = ts.max()
    if latest >= DISCOVERY_END_EXCLUSIVE:
        raise PartitionBreach(f"{what}: row at {latest.isoformat()} is not strictly before "
                              f"{DISCOVERY_END_EXCLUSIVE.date()} (confirmation reserve / final holdout fence)")
