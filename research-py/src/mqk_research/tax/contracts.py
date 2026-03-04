from __future__ import annotations

from dataclasses import dataclass
from typing import List


@dataclass(frozen=True)
class FillsCsvContractV1:
    required_columns: List[str] = None

    def normalized(self) -> "FillsCsvContractV1":
        return FillsCsvContractV1(
            required_columns=self.required_columns or [
                "symbol",
                "fill_ts",
                "side",
                "qty",
                "price",
            ]
        )


@dataclass(frozen=True)
class EquityCurveCsvContractV1:
    required_columns: List[str] = None

    def normalized(self) -> "EquityCurveCsvContractV1":
        return EquityCurveCsvContractV1(
            required_columns=self.required_columns or [
                "ts",
                "equity",
            ]
        )
