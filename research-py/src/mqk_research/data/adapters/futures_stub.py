from __future__ import annotations

from dataclasses import dataclass
from datetime import timezone
from typing import Optional

import pandas as pd


@dataclass(frozen=True)
class FuturesHistoryQuery:
    """
    NOT SUPPORTED. Futures data requires a Postgres futures schema (continuous/rolled
    series, roll schedules) and an ingestion pipeline that do not exist in this codebase.

    Determinism rule (for when this is eventually implemented): all queries must be
    ASOF-scoped and sourced exclusively from Postgres — no live API calls at research time.
    """
    root: str                  # e.g., "ES"
    asof_utc: pd.Timestamp
    start_utc: pd.Timestamp
    end_utc: pd.Timestamp
    contract: Optional[str] = None  # e.g., "ESM2026"
    roll_rule: str = "front_month"


def load_futures_history_pg(*, query: FuturesHistoryQuery) -> pd.DataFrame:
    """
    NOT SUPPORTED. Raises NotImplementedError unconditionally.

    Prerequisites before this can be implemented:
      1. Postgres futures schema (continuous series, roll schedule, contract metadata)
      2. Futures data ingestion pipeline
      3. ASOF-scoped roll-adjusted query contract defined and migration applied

    Do not call this function. The CLI refuses FUTURES asset_class at the routing
    boundary before this adapter is reached.
    """
    asof = pd.to_datetime(query.asof_utc, utc=True).to_pydatetime().astimezone(timezone.utc)
    raise NotImplementedError(
        "Futures pipeline is not supported.\n"
        f"  root={query.root} contract={query.contract} roll_rule={query.roll_rule}\n"
        f"  asof_utc={asof.isoformat()}\n"
        "Required: Postgres futures schema + ingestion pipeline (neither exists). "
        "The research CLI refuses FUTURES asset_class before reaching this adapter."
    )
