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
import math
from collections.abc import Mapping as ABCMapping
from dataclasses import dataclass, field
from enum import Enum
from types import MappingProxyType
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


def _canonical_value(obj: Any) -> Any:
    """Recursively validates and DEEPLY FREEZES a value for inclusion in an
    economic identity. Only explicit, finite, JSON-native types are
    accepted -- no `default=str` fallback, so an unsupported object can
    never enter an identity hash through its (possibly unstable, possibly
    process-specific) string representation.

    Returns a deeply immutable structure: nested lists/tuples become
    tuples, nested dicts become MappingProxyType. A single outer
    MappingProxyType is NOT enough -- a mutable list or dict *value* nested
    inside it is still reachable and mutable unless every level is frozen
    (FW-C2-R1). Idempotent: accepts its own previously-frozen output
    (MappingProxyType/tuple) as input, so re-canonicalizing an
    already-canonical structure (e.g. when population.py builds a
    HypothesisGrammar from an already-frozen parameter_grids value) is a
    safe no-op, not a spurious type-rejection."""
    if obj is None or isinstance(obj, str):
        return obj
    if isinstance(obj, bool):
        return obj
    if isinstance(obj, int):
        return obj
    if isinstance(obj, float):
        if not math.isfinite(obj):
            raise GrammarError(f"non-finite float is not an allowed economic parameter value: {obj!r}")
        return obj
    if isinstance(obj, (list, tuple)):
        return tuple(_canonical_value(v) for v in obj)
    if isinstance(obj, ABCMapping):
        canon: Dict[str, Any] = {}
        for k, v in obj.items():
            if not isinstance(k, str):
                raise GrammarError(
                    f"only string keys are allowed in economic parameters, got {type(k).__name__}: {k!r}"
                )
            canon[k] = _canonical_value(v)
        return MappingProxyType(canon)
    raise GrammarError(f"unsupported economic parameter value type {type(obj).__name__}: {obj!r}")


def _thaw(obj: Any) -> Any:
    """Inverse of the freezing performed by `_canonical_value`: recursively
    converts MappingProxyType -> dict and tuple -> list, producing a plain
    JSON-native structure. Used ONLY at the boundary where a plain,
    mutable-looking (but freshly-built, not aliased to any stored object)
    structure is actually required: `economic_fields()`, declaration-id
    payload assembly, and `trial_identity_for()`. Byte-identical JSON
    output to the pre-freeze representation for any previously-valid
    input, so existing fingerprints/ids are unaffected."""
    if isinstance(obj, MappingProxyType):
        return {k: _thaw(v) for k, v in obj.items()}
    if isinstance(obj, tuple):
        return [_thaw(v) for v in obj]
    return obj


def _canonical_json(obj: Any) -> str:
    return json.dumps(_thaw(_canonical_value(obj)), sort_keys=True, separators=(",", ":"))


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

    def __post_init__(self) -> None:
        """A frozen dataclass only prevents REASSIGNING `self.parameters`;
        it does nothing to stop a caller from mutating the same mapping
        object (or, critically, a mutable value NESTED inside it) after
        construction, which would silently change a supposedly fixed
        economic identity (FW-C2-R1). `_canonical_value` freezes every
        level of the structure -- not just the outer mapping -- into
        MappingProxyType/tuple, so `grammar.parameters["n"].append(3)` has
        no `.append` to call; it also validates/normalizes every value
        (rejecting non-finite floats and unsupported types) at construction
        time rather than only when a fingerprint happens to be requested
        later."""
        object.__setattr__(self, "parameters", _canonical_value(dict(self.parameters)))
        object.__setattr__(self, "data_inputs", tuple(self.data_inputs))

    def economic_fields(self) -> Dict[str, Any]:
        """Every field is economically load-bearing for identity; this is
        kept as a separate method (rather than always using every dataclass
        field) so a future purely-cosmetic field can be added without
        silently changing every existing fingerprint."""
        return {
            "mechanism_family": self.mechanism_family.value,
            "direction": self.direction.value,
            "asset_class": self.asset_class.value,
            "parameters": _thaw(self.parameters),
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
