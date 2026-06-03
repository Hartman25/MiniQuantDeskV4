"""
EXP-ENGINE-CORE-01 — abstract base scanner interface (scanner-only, Stage 1).

Subclasses implement scan() to produce candidate records.
No broker adapter, OMS, or execution orchestrator imports are permitted here.
"""
from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any


class ScannerBase(ABC):
    """
    Base class for experimental scanner lanes.

    Contract:
    - scan() returns a list of candidate dicts (use candidate_journal.build_candidate_record).
    - scan() must never submit orders, write to OMS tables, or call broker endpoints.
    - All output is journaled by the runner; scan() only produces records.
    """

    @property
    @abstractmethod
    def engine_id(self) -> str:
        ...

    @property
    @abstractmethod
    def lane_id(self) -> str:
        ...

    @abstractmethod
    def scan(self) -> list[dict[str, Any]]:
        """
        Run a single scan pass and return candidate records.
        Must not place orders, call broker APIs, or write to OMS tables.
        """
        ...
