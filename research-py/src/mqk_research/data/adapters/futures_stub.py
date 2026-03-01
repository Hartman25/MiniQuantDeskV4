from __future__ import annotations

from dataclasses import dataclass
from datetime import timezone
from typing import Optional

import pandas as pd


@dataclass(frozen=True)
class FuturesHistoryQuery:
    """
    Phase 2 stub. Future: query continuous/rolled futures series from Postgres.
    Determinism rule: all queries must be ASOF-scoped and come only from Postgres.
    """
    root: str                  # e.g., "ES"
    asof_utc: pd.Timestamp
    start_utc: pd.Timestamp
    end_utc: pd.Timestamp
    contract: Optional[str] = None  # e.g., "ESM2026"
    roll_rule: str = "front_month"  # placeholder


def load_futures_history_pg(*, query: FuturesHistoryQuery) -> pd.DataFrame:
    """
    Stub adapter. Must be implemented after Postgres futures schema exists.
    """
    asof = pd.to_datetime(query.asof_utc, utc=True).to_pydatetime().astimezone(timezone.utc)
    raise RuntimeError(
        "Futures adapter not implemented (Phase 2 stub).\n"
        f"  root={query.root} contract={query.contract} roll_rule={query.roll_rule}\n"
        f"  asof_utc={asof.isoformat()}\n"
        "Next: define Postgres futures schema + ingestion, then implement adapter."
    )