"""
RESEARCH-POINT-IN-TIME-UNIVERSE-01

Canonical point-in-time universe contract: `UniverseMembershipRecord` +
`UniverseSpec`, with a `members_as_of(timestamp)` query that fails closed
when historical membership is requested outside the universe's declared
coverage window -- there is no "use the current list" fallback.

No provider/web integration here: `UniverseSpec` accepts caller-supplied
canonical membership data only. Distinguishes "not a member then" (the
requested timestamp IS covered, but no membership record was active) from
"membership unknown" (the requested timestamp falls outside the declared
coverage window entirely) -- the latter always raises `UniverseCoverageError`
rather than silently returning an empty set.
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Dict, FrozenSet, List, Optional

import pandas as pd

from mqk_research.exp_distributed.hashing import short_hash

UNIVERSE_CONTRACT_SCHEMA_VERSION = "universe_contract_v1"


class UniverseCoverageError(ValueError):
    """Raised when `members_as_of` is asked about a timestamp outside the
    universe's declared point-in-time coverage window -- membership is
    UNKNOWN, never silently inferred from the current/most-recent list."""


class UniverseMembershipViolation(ValueError):
    """Raised when a factor observation's symbol was not a member of the
    bound universe at its own observation timestamp."""


def _parse_ts(value: str, *, field_name: str = "timestamp") -> datetime:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{field_name} must be a non-empty ISO-8601 UTC timestamp string")
    normalized = value.strip().replace("Z", "+00:00")
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as exc:
        raise ValueError(f"{field_name} is not a valid ISO-8601 timestamp: {value!r}") from exc
    if parsed.tzinfo is None:
        raise ValueError(f"{field_name} must carry an explicit UTC offset/Z suffix: {value!r}")
    return parsed


@dataclass(frozen=True)
class UniverseMembershipRecord:
    """One point-in-time membership window for one symbol.

    `effective_through_utc=None` means the record is still open (no known
    removal) as of the universe's own `coverage_end_utc` -- it does NOT mean
    "forever"; queries are always bounded by the enclosing UniverseSpec's
    coverage window.
    """

    symbol: str
    effective_from_utc: str
    effective_through_utc: Optional[str] = None
    source_identity: Dict[str, Any] = field(default_factory=dict)
    inclusion_reason: Optional[str] = None
    exclusion_reason: Optional[str] = None

    def validate(self) -> None:
        if not self.symbol.strip():
            raise ValueError("UniverseMembershipRecord.symbol is required")
        from_ts = _parse_ts(self.effective_from_utc, field_name="effective_from_utc")
        if self.effective_through_utc is not None:
            through_ts = _parse_ts(self.effective_through_utc, field_name="effective_through_utc")
            if through_ts <= from_ts:
                raise ValueError(
                    "UniverseMembershipRecord.effective_through_utc must be strictly after effective_from_utc"
                )
        if not isinstance(self.source_identity, dict):
            raise ValueError("UniverseMembershipRecord.source_identity must be a dict")

    def identity_payload(self) -> Dict[str, Any]:
        return {
            "symbol": self.symbol,
            "effective_from_utc": self.effective_from_utc,
            "effective_through_utc": self.effective_through_utc,
            "source_identity": copy.deepcopy(self.source_identity),
            "inclusion_reason": self.inclusion_reason,
            "exclusion_reason": self.exclusion_reason,
        }


@dataclass(frozen=True)
class UniverseSpec:
    """Identity-bearing, caller-supplied point-in-time universe.

    `coverage_start_utc`/`coverage_end_utc` (half-open, `coverage_end_utc`
    exclusive) declare the window over which THIS snapshot's membership data
    is asserted complete. A `members_as_of` query outside that window always
    fails closed (`UniverseCoverageError`) -- it is never answered by
    falling back to the most recent/current membership.
    """

    universe_name: str
    universe_protocol_version: str
    coverage_start_utc: str
    coverage_end_utc: str
    members: List[UniverseMembershipRecord] = field(default_factory=list)
    source_identity: Dict[str, Any] = field(default_factory=dict)

    def validate(self) -> None:
        if not self.universe_name.strip():
            raise ValueError("UniverseSpec.universe_name is required")
        if not self.universe_protocol_version.strip():
            raise ValueError("UniverseSpec.universe_protocol_version is required")
        start_ts = _parse_ts(self.coverage_start_utc, field_name="coverage_start_utc")
        end_ts = _parse_ts(self.coverage_end_utc, field_name="coverage_end_utc")
        if start_ts >= end_ts:
            raise ValueError("UniverseSpec.coverage_start_utc must be before coverage_end_utc")
        if not isinstance(self.source_identity, dict):
            raise ValueError("UniverseSpec.source_identity must be a dict")
        for record in self.members:
            record.validate()
        self._check_no_overlapping_windows()

    def _check_no_overlapping_windows(self) -> None:
        by_symbol: Dict[str, List[UniverseMembershipRecord]] = {}
        for record in self.members:
            by_symbol.setdefault(record.symbol, []).append(record)
        for symbol, records in by_symbol.items():
            ordered = sorted(records, key=lambda r: _parse_ts(r.effective_from_utc))
            for prev, curr in zip(ordered, ordered[1:]):
                prev_through = _parse_ts(prev.effective_through_utc) if prev.effective_through_utc else None
                curr_from = _parse_ts(curr.effective_from_utc)
                if prev_through is None or curr_from < prev_through:
                    raise ValueError(
                        f"UniverseSpec has overlapping/ambiguous membership windows for symbol {symbol!r}"
                    )

    def identity_payload(self) -> Dict[str, Any]:
        sorted_members = sorted(
            (m.identity_payload() for m in self.members),
            key=lambda m: (m["symbol"], m["effective_from_utc"], m["effective_through_utc"] or ""),
        )
        return {
            "schema_version": UNIVERSE_CONTRACT_SCHEMA_VERSION,
            "universe_name": self.universe_name,
            "universe_protocol_version": self.universe_protocol_version,
            "coverage_start_utc": self.coverage_start_utc,
            "coverage_end_utc": self.coverage_end_utc,
            "members": sorted_members,
            "source_identity": copy.deepcopy(self.source_identity),
        }

    def compute_universe_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)

    def members_as_of(self, timestamp_utc: str) -> FrozenSet[str]:
        """Point-in-time membership. Raises `UniverseCoverageError` if
        `timestamp_utc` falls outside [coverage_start_utc, coverage_end_utc)
        -- membership is UNKNOWN there, never inferred from current data."""
        self.validate()
        ts = _parse_ts(timestamp_utc)
        start_ts = _parse_ts(self.coverage_start_utc)
        end_ts = _parse_ts(self.coverage_end_utc)
        if ts < start_ts or ts >= end_ts:
            raise UniverseCoverageError(
                f"timestamp {timestamp_utc!r} is outside declared coverage "
                f"[{self.coverage_start_utc}, {self.coverage_end_utc})"
            )
        active: set[str] = set()
        for record in self.members:
            m_from = _parse_ts(record.effective_from_utc)
            m_through = _parse_ts(record.effective_through_utc) if record.effective_through_utc else None
            if m_from <= ts and (m_through is None or ts < m_through):
                active.add(record.symbol)
        return frozenset(active)


def universe_identity_binding(universe: UniverseSpec) -> Dict[str, Any]:
    """Canonical `universe_identity` payload for binding a
    `mqk_research.factors.contracts.FactorEvaluationSpec` to this universe.
    A semantic membership change changes `universe_id`, which -- because
    FactorEvaluationSpec.universe_identity already contributes to
    `evaluation_id` -- automatically changes evaluation-slice identity
    without requiring any change to the Patch A contract."""
    return {
        "universe_id": universe.compute_universe_id(),
        "universe_protocol_version": universe.universe_protocol_version,
    }


def assert_observations_within_universe(
    observations: pd.DataFrame,
    universe: UniverseSpec,
    *,
    symbol_col: str = "symbol",
    period_col: str = "period_ts_utc",
) -> None:
    """Fail closed if any (symbol, period) row was not an active universe
    member at its own observation timestamp. Proves that a factor evaluation
    bound to `universe` never silently scored a symbol outside it -- no
    survivorship shortcut, no current-list fallback."""
    for _, row in observations.iterrows():
        symbol = row[symbol_col]
        period_ts = row[period_col]
        active = universe.members_as_of(period_ts)
        if symbol not in active:
            raise UniverseMembershipViolation(
                f"symbol {symbol!r} was not a member of universe {universe.universe_name!r} "
                f"at {period_ts!r}"
            )
