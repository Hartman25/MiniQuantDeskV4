from __future__ import annotations

from dataclasses import dataclass
from datetime import timezone
from typing import Optional

import pandas as pd


@dataclass(frozen=True)
class OptionsChainQuery:
    """
    NOT SUPPORTED. Options data requires a Postgres options schema (chains, greeks, IV)
    and an ingestion pipeline that do not exist in this codebase.

    Determinism rule (for when this is eventually implemented): all queries must be
    ASOF-scoped and sourced exclusively from Postgres — no live API calls at research time.
    """
    symbol: str
    asof_utc: pd.Timestamp
    expiry_utc: Optional[pd.Timestamp] = None


def load_options_chain_pg(*, query: OptionsChainQuery) -> pd.DataFrame:
    """
    NOT SUPPORTED. Raises NotImplementedError unconditionally.

    Prerequisites before this can be implemented:
      1. Postgres options schema (option_chains / greeks / IV surface tables)
      2. Options data ingestion pipeline
      3. ASOF-scoped query contract defined and migration applied

    Do not call this function. The CLI refuses OPTIONS asset_class at the routing
    boundary before this adapter is reached.
    """
    asof = pd.to_datetime(query.asof_utc, utc=True).to_pydatetime().astimezone(timezone.utc)
    raise NotImplementedError(
        "Options pipeline is not supported.\n"
        f"  symbol={query.symbol}\n"
        f"  asof_utc={asof.isoformat()}\n"
        "Required: Postgres options schema + ingestion pipeline (neither exists). "
        "The research CLI refuses OPTIONS asset_class before reaching this adapter."
    )
