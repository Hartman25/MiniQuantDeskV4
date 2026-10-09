"""
Bounded, deterministic, asset-neutral hypothesis-population grammar
(Component C of V4-MQD-FOUR-WAY-ASSET-NEUTRAL-INTEGRATION-ISOLATED-01).

Dry-run only: nothing in this module registers a trial, runs a backtest,
calls a data provider, or reads/writes the Research registry DB. It
produces an in-memory, finite, fingerprinted candidate population that a
SEPARATELY AUTHORIZED future campaign could feed into the existing
`ResearchResultStore.register_hypothesis` / `register_trial` seam
(research-py/src/mqk_research/exp_distributed/storage.py) unchanged.
"""

from __future__ import annotations

import json
import hashlib
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, Mapping, Tuple


class MechanismFamily(str, Enum):
    """Future comprehensive-research capability categories (mission
    Section 10). Listing a family here is not permission to execute it."""

    TREND_FOLLOWING = "trend_following"
    MOMENTUM = "momentum"
    PULLBACK_MEAN_REVERSION = "pullback_mean_reversion"
    PRICE_ACTION = "price_action"
    BREAKOUT = "breakout"
    REVERSAL = "reversal"
    GAP = "gap"
    VOLATILITY = "volatility"
    VOLUME_LIQUIDITY = "volume_liquidity"
    SEASONAL = "seasonal"
    RELATIVE_VALUE = "relative_value"


class Direction(str, Enum):
    LONG = "long"
    SHORT = "short"
    LONG_SHORT = "long_short"
    UNSUPPORTED = "unsupported"


class AssetClassTag(str, Enum):
    """Mirrors core-rs mqk-schemas::AssetClass naming (Equity/Option/Future/
    Crypto/Forex) plus an explicit UNKNOWN fail-closed default. ETF is not a
    distinct class on either side — it is covered by EQUITY, matching the
    production registry's own treatment."""

    EQUITY = "equity"
    OPTION = "option"
    FUTURE = "future"
    CRYPTO = "crypto"
    FOREX = "forex"
    UNKNOWN = "unknown"


# The only asset class this mission authorizes operational preparation for.
# Every other tag is represented in the grammar (multi-asset seam) but is
# always classified unsupported by the population generator — never
# silently coerced to this default.
OPERATIONAL_ASSET_CLASSES: frozenset[AssetClassTag] = frozenset({AssetClassTag.EQUITY})


class GrammarError(ValueError):
    """Fail-closed refusal: malformed, unbounded, or undeclared grammar input."""


def _canonical_json(obj: Any) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), default=str)


def _sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


@dataclass(frozen=True)
class HypothesisGrammar:
    """
    One prospective, finite, result-independent hypothesis realization.

    Every field here is an ECONOMIC/IDENTITY input declared before any
    execution — there is no field for a result, a return, a P&L, or a
    rank. That is enforced structurally (see
    `test_strategy_mining_population.py::test_hypothesis_grammar_has_no_result_field`),
    not just by convention.
    """

    mechanism_family: MechanismFamily
    direction: Direction
    asset_class: AssetClassTag
    parameters: Mapping[str, Any]
    timeframe: str
    data_inputs: Tuple[str, ...]
    universe_requirement: str
    session_calendar_contract: str
    sizing_contract: str
    execution_model_contract: str
    cost_model_contract: str
    risk_model_requirement: str
    point_in_time_universe_requirement: str
    historical_evidence_partition: str

    def economic_fields(self) -> Dict[str, Any]:
        """Every field is economically load-bearing for identity; this is
        kept as a separate method (rather than always using every dataclass
        field) so a future purely-cosmetic field can be added without
        silently changing every existing fingerprint."""
        return {
            "mechanism_family": self.mechanism_family.value,
            "direction": self.direction.value,
            "asset_class": self.asset_class.value,
            "parameters": dict(self.parameters),
            "timeframe": self.timeframe,
            "data_inputs": sorted(self.data_inputs),
            "universe_requirement": self.universe_requirement,
            "session_calendar_contract": self.session_calendar_contract,
            "sizing_contract": self.sizing_contract,
            "execution_model_contract": self.execution_model_contract,
            "cost_model_contract": self.cost_model_contract,
            "risk_model_requirement": self.risk_model_requirement,
            "point_in_time_universe_requirement": self.point_in_time_universe_requirement,
            "historical_evidence_partition": self.historical_evidence_partition,
        }

    def semantic_fingerprint(self) -> str:
        """Stable regardless of dict/field insertion order (canonical JSON,
        sorted keys). Changing any economic field changes this; changing
        nothing about field insertion order does not."""
        return _sha256_hex(_canonical_json(self.economic_fields()))

    def is_operationally_supported(self) -> bool:
        return self.asset_class in OPERATIONAL_ASSET_CLASSES and self.direction != Direction.UNSUPPORTED

    def compatibility_status(self) -> str:
        if self.asset_class not in OPERATIONAL_ASSET_CLASSES:
            return "unsupported_asset_class"
        if self.direction == Direction.UNSUPPORTED:
            return "unsupported_direction"
        return "compatible"
