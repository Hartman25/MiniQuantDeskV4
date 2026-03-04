from __future__ import annotations

from dataclasses import dataclass
from typing import List


@dataclass(frozen=True)
class ShadowIntentsContractV1:
    """
    Shadow intents are strategy outputs captured WITHOUT execution.
    This contract is intentionally small and stable.

    Required columns in shadow_intents.csv:
      - run_id: string (research/run identifier)
      - symbol: string
      - decision_ts: string (UTC timestamp)  # aligns to bar end_ts or decision time
      - intent: string  # e.g. "BUY", "SELL", "HOLD" (or your enum)
      - horizon_bars: int  # how far forward to label

    Optional columns:
      - score: float (recommended)
      - qty: float/int
      - reason_code: string
      - policy_hash: string
      - feature_schema_hash: string
      - signal_pack_id: string
    """
    required_columns: List[str] = None

    def normalized(self) -> "ShadowIntentsContractV1":
        return ShadowIntentsContractV1(
            required_columns=self.required_columns or [
                "run_id",
                "symbol",
                "decision_ts",
                "intent",
                "horizon_bars",
            ]
        )
