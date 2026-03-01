from __future__ import annotations

from dataclasses import dataclass
from datetime import timezone
from typing import Optional

import pandas as pd


@dataclass(frozen=True)
class OptionsChainQuery:
    """
    Phase 2 stub. Future: query option chains/greeks/IV from Postgres tables.
    Determinism rule: all queries must be ASOF-scoped and come only from Postgres.
    """
    symbol: str
    asof_utc: pd.Timestamp
    expiry_utc: Optional[pd.Timestamp] = None


def load_options_chain_pg(*, query: OptionsChainQuery) -> pd.DataFrame:
    """
    Stub adapter. Must be implemented after Postgres options schema exists.
    """
    asof = pd.to_datetime(query.asof_utc, utc=True).to_pydatetime().astimezone(timezone.utc)
    raise RuntimeError(
        "Options adapter not implemented (Phase 2 stub).\n"
        f"  symbol={query.symbol}\n"
        f"  asof_utc={asof.isoformat()}\n"
        "Next: define Postgres options schema + ingestion, then implement adapter."
    )