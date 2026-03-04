from __future__ import annotations

from dataclasses import dataclass
from typing import List


@dataclass(frozen=True)
class SignalPackContractV1:
    """Stable research -> backtest boundary."""
    required_columns: List[str] = None

    def normalized(self) -> "SignalPackContractV1":
        return SignalPackContractV1(
            required_columns=self.required_columns or [
                "ts",          # UTC timestamp string (decision time)
                "symbol",
                "signal",      # float: score or target position; interpretation defined by policy
                "horizon_bars",
                "policy_hash",
                "run_id",
            ]
        )
