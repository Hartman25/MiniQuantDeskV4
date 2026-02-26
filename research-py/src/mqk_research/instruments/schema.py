from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Optional


class AssetClass(str, Enum):
    EQUITY = "EQUITY"
    OPTION = "OPTION"
    FUTURE = "FUTURE"


@dataclass(frozen=True)
class Instrument:
    instrument_id: str
    symbol: str
    asset_class: AssetClass = AssetClass.EQUITY
    exchange: Optional[str] = None
    currency: str = "USD"