"""
Finite, deterministic, result-independent hypothesis-population generator
(Component C). Dry-run only — see module docstring in `grammar.py` for the
full isolation contract. This module never imports the Research registry
(`exp_distributed.storage`), a broker/daemon/Promotion module, or any
network/provider client; see
`test_strategy_mining_population.py::test_module_touches_no_registry_broker_or_network`.

Predeclaration discipline (mirrors the existing experiment convention, e.g.
`research-py/experiments/*/test_predeclaration.py`): the caller declares the
exact population shape up front (`PopulationDeclaration`); generation then
asserts the realized raw count equals the predeclared cardinality exactly,
before any deduplication or compatibility filtering — a structural "no
omitted / no extra member" proof, not just a test-level one.
"""

from __future__ import annotations

import itertools
import math
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import Any, Dict, List, Mapping, Sequence, Tuple

from mqk_research.strategy_mining.grammar import (
    AssetClassTag,
    Direction,
    GrammarError,
    HypothesisGrammar,
    MechanismFamily,
    _canonical_json,
    _canonical_value,
    _sha256_hex,
    _thaw,
)


@dataclass(frozen=True)
class PopulationDeclaration:
    """Exact, finite population shape, predeclared before generation."""

    mechanism_families: Tuple[MechanismFamily, ...]
    directions: Tuple[Direction, ...]
    asset_classes: Tuple[AssetClassTag, ...]
    parameter_grids: Mapping[MechanismFamily, Mapping[str, Tuple[Any, ...]]]
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
    max_population_size: int = 2_000

    def __post_init__(self) -> None:
        """Same root cause as HypothesisGrammar's own fix (C2): a frozen
        dataclass does not stop a caller from mutating a nested mutable
        mapping/list after construction. The PREVIOUS fix only wrapped the
        outer grid dict in MappingProxyType and the per-axis container in
        `tuple(values)` -- an individual axis VALUE that is itself a list
        or dict (e.g. `parameter_grids[family]["n"] = ([1, 2], [3, 4])`)
        was still a mutable object reachable via
        `decl.parameter_grids[family]["n"][0].append(3)` (FW-C2-R1).
        `_canonical_value` now freezes every level, recursively, including
        each individual axis value."""
        object.__setattr__(self, "mechanism_families", tuple(self.mechanism_families))
        object.__setattr__(self, "directions", tuple(self.directions))
        object.__setattr__(self, "asset_classes", tuple(self.asset_classes))
        object.__setattr__(self, "data_inputs", tuple(self.data_inputs))
        normalized_grids = {
            family: MappingProxyType({name: _canonical_value(tuple(values)) for name, values in grid.items()})
            for family, grid in self.parameter_grids.items()
        }
        object.__setattr__(self, "parameter_grids", MappingProxyType(normalized_grids))

    def validate(self) -> None:
        """Every contract field is mandatory and must be an explicit,
        non-empty declaration. "none" is a legitimate explicit value (e.g.
        risk_model_requirement="none"); an empty/missing string is not —
        that is a caller who forgot to declare the contract, and must fail
        closed rather than silently produce candidates with an undeclared
        calendar/execution/sizing/cost/risk/universe/partition model."""
        required_str_fields = {
            "timeframe": self.timeframe,
            "universe_requirement": self.universe_requirement,
            "session_calendar_contract": self.session_calendar_contract,
            "sizing_contract": self.sizing_contract,
            "execution_model_contract": self.execution_model_contract,
            "cost_model_contract": self.cost_model_contract,
            "risk_model_requirement": self.risk_model_requirement,
            "point_in_time_universe_requirement": self.point_in_time_universe_requirement,
            "historical_evidence_partition": self.historical_evidence_partition,
        }
        for name, value in required_str_fields.items():
            if not isinstance(value, str) or not value.strip():
                raise GrammarError(f"{name} is missing/empty — every contract field must be explicitly declared")
        if not self.data_inputs or any(not isinstance(d, str) or not d.strip() for d in self.data_inputs):
            raise GrammarError("data_inputs must be a non-empty tuple of non-empty strings")
        if not self.mechanism_families:
            raise GrammarError("mechanism_families must be non-empty")
        if not self.directions:
            raise GrammarError("directions must be non-empty")
        if not self.asset_classes:
            raise GrammarError("asset_classes must be non-empty")

        if not all(isinstance(f, MechanismFamily) for f in self.mechanism_families):
            raise GrammarError("every mechanism_families element must be a MechanismFamily enum member")
        if not all(isinstance(d, Direction) for d in self.directions):
            raise GrammarError("every directions element must be a Direction enum member")
        if not all(isinstance(a, AssetClassTag) for a in self.asset_classes):
            raise GrammarError("every asset_classes element must be an AssetClassTag enum member")

        missing_grids = [f for f in self.mechanism_families if f not in self.parameter_grids]
        if missing_grids:
            raise GrammarError(
                f"mechanism_families {[f.value for f in missing_grids]} have no explicit parameter_grids entry "
                "-- declare it (an empty {} is a valid explicit 'no parameters' declaration, but the key itself "
                "must be present; a silently-missing key is a caller who forgot to declare the contract)"
            )
        extra_grids = [f for f in self.parameter_grids if f not in self.mechanism_families]
        if extra_grids:
            raise GrammarError(
                f"parameter_grids declares {[f.value for f in extra_grids]}, which are not in mechanism_families "
                "-- undeclared/transport-only metadata must not exist (it would change declaration identity "
                "while generating the identical candidate set, FW-C3 addendum)"
            )
        for family, grid in self.parameter_grids.items():
            for name, values in grid.items():
                if not isinstance(name, str) or not name.strip():
                    raise GrammarError(f"parameter domain name for {family.value} must be a non-empty string, got {name!r}")
                if len(values) == 0:
                    raise GrammarError(
                        f"parameter domain {family.value}.{name} is empty — an axis with zero declared values "
                        "yields zero real combinations, not one (remove the axis, or supply at least one value)"
                    )

        if isinstance(self.max_population_size, bool):
            raise GrammarError("max_population_size must not be a bool")
        if not isinstance(self.max_population_size, (int, float)) or not math.isfinite(float(self.max_population_size)):
            raise GrammarError("max_population_size must be a finite number")
        if not float(self.max_population_size).is_integer() or int(self.max_population_size) <= 0:
            raise GrammarError("max_population_size must be a positive integer")

    def _param_combo_count(self, family: MechanismFamily) -> int:
        """An empty grid `{}` (no axes at all) correctly stays at the
        initial 1 (one parameterless variant) since the loop below never
        executes. A NAMED axis with zero values (`"n": ()`) must drive the
        count to 0 -- `max(len(values), 1)` previously treated it as 1,
        silently reporting a wrong cardinality (FW-C3-R7); `validate()`
        now rejects this case explicitly before it would ever reach here,
        but this function must also compute the honest number on its own."""
        grid = self.parameter_grids.get(family, {})
        count = 1
        for values in grid.values():
            count *= len(values)
        return count

    def declared_cardinality(self) -> int:
        total = 0
        for family in self.mechanism_families:
            total += self._param_combo_count(family) * len(self.directions) * len(self.asset_classes)
        return total

    def _param_combos(self, family: MechanismFamily) -> List[Dict[str, Any]]:
        grid = self.parameter_grids.get(family, {})
        if not grid:
            return [{}]
        names = sorted(grid.keys())  # deterministic regardless of dict insertion order
        value_lists = [grid[name] for name in names]
        for name, values in zip(names, value_lists):
            if len(set(_canonical_json(v) for v in values)) != len(values):
                raise GrammarError(f"parameter domain {family.value}.{name} contains duplicate values")
        return [dict(zip(names, combo)) for combo in itertools.product(*value_lists)]


@dataclass(frozen=True)
class PopulationManifest:
    declaration_id: str
    declared_cardinality: int
    raw_count: int
    duplicate_count: int
    compatible_count: int
    unsupported_count: int
    manifest_id: str
    items: Tuple[Tuple[HypothesisGrammar, str], ...]  # (grammar, compatibility_status), fingerprint-sorted
    max_population_size: int  # administrative/resource bound only -- not identity-bearing (C4)


def _declaration_id(decl: PopulationDeclaration) -> str:
    payload = {
        "mechanism_families": sorted(m.value for m in decl.mechanism_families),
        "directions": sorted(d.value for d in decl.directions),
        "asset_classes": sorted(a.value for a in decl.asset_classes),
        "parameter_grids": {
            family.value: {name: _thaw(values) for name, values in grid.items()}
            for family, grid in decl.parameter_grids.items()
        },
        "timeframe": decl.timeframe,
        "data_inputs": sorted(decl.data_inputs),
        "universe_requirement": decl.universe_requirement,
        "session_calendar_contract": decl.session_calendar_contract,
        "sizing_contract": decl.sizing_contract,
        "execution_model_contract": decl.execution_model_contract,
        "cost_model_contract": decl.cost_model_contract,
        "risk_model_requirement": decl.risk_model_requirement,
        "point_in_time_universe_requirement": decl.point_in_time_universe_requirement,
        "historical_evidence_partition": decl.historical_evidence_partition,
        # max_population_size is deliberately EXCLUDED: it is a resource/
        # execution-layout bound, not an economic parameter (C4). Changing
        # only the cap while every economic field stays identical must not
        # manufacture a different declaration/manifest identity.
    }
    return _sha256_hex(_canonical_json(payload))


def generate_population(decl: PopulationDeclaration) -> PopulationManifest:
    decl.validate()
    declared = decl.declared_cardinality()
    if declared <= 0:
        raise GrammarError("predeclared population is empty — nothing to generate")
    if declared > decl.max_population_size:
        raise GrammarError(
            f"predeclared population ({declared}) exceeds max_population_size "
            f"({decl.max_population_size}); narrow the grammar or raise the bound explicitly"
        )

    raw: List[HypothesisGrammar] = []
    for family in decl.mechanism_families:
        for params in decl._param_combos(family):
            for direction in decl.directions:
                for asset_class in decl.asset_classes:
                    raw.append(
                        HypothesisGrammar(
                            mechanism_family=family,
                            direction=direction,
                            asset_class=asset_class,
                            parameters=params,
                            timeframe=decl.timeframe,
                            data_inputs=decl.data_inputs,
                            universe_requirement=decl.universe_requirement,
                            session_calendar_contract=decl.session_calendar_contract,
                            sizing_contract=decl.sizing_contract,
                            execution_model_contract=decl.execution_model_contract,
                            cost_model_contract=decl.cost_model_contract,
                            risk_model_requirement=decl.risk_model_requirement,
                            point_in_time_universe_requirement=decl.point_in_time_universe_requirement,
                            historical_evidence_partition=decl.historical_evidence_partition,
                        )
                    )

    # Structural no-omission / no-extra-member proof: the raw (pre-dedup)
    # count must equal exactly what was predeclared.
    if len(raw) != declared:
        raise GrammarError(
            f"generated {len(raw)} raw candidates but {declared} were predeclared "
            "(generator/declaration mismatch — refusing to return a silently wrong population)"
        )

    # Deterministic ordering independent of input list order: sort by fingerprint.
    raw.sort(key=lambda g: g.semantic_fingerprint())

    seen: Dict[str, HypothesisGrammar] = {}
    duplicate_count = 0
    deduped: List[HypothesisGrammar] = []
    for g in raw:
        fp = g.semantic_fingerprint()
        if fp in seen:
            duplicate_count += 1
            continue
        seen[fp] = g
        deduped.append(g)

    items: List[Tuple[HypothesisGrammar, str]] = [(g, g.compatibility_status()) for g in deduped]
    compatible_count = sum(1 for _, status in items if status == "compatible")
    unsupported_count = len(items) - compatible_count

    decl_id = _declaration_id(decl)
    manifest_id = _sha256_hex(
        _canonical_json(
            {
                "declaration_id": decl_id,
                "items": [(g.semantic_fingerprint(), status) for g, status in items],
            }
        )
    )

    return PopulationManifest(
        declaration_id=decl_id,
        declared_cardinality=declared,
        raw_count=len(raw),
        duplicate_count=duplicate_count,
        compatible_count=compatible_count,
        unsupported_count=unsupported_count,
        manifest_id=manifest_id,
        items=tuple(items),
        max_population_size=int(decl.max_population_size),
    )


def trial_identity_for(
    grammar: HypothesisGrammar,
    *,
    experiment_id: str,
    hypothesis_id: str,
) -> Dict[str, Any]:
    """
    Builds the `identity` payload shape that `ResearchResultStore.register_trial`
    (research-py/src/mqk_research/exp_distributed/storage.py) would accept for
    this candidate, for a SEPARATELY AUTHORIZED future campaign to use.

    This function does not import or call the registry — it only shapes data.
    Schema-compatibility with the real registration seam is proven in
    `test_strategy_mining_population.py::test_trial_identity_round_trips_through_the_real_registry`,
    which imports and calls the real `ResearchResultStore` against an
    isolated tmp-path database, never the production Research DB.

    Second-sweep addition: rejects a blank experiment_id/hypothesis_id
    here, at the shaping boundary, rather than relying on the (unmodified,
    out-of-scope) registry to catch it later.
    """
    if not experiment_id or not experiment_id.strip():
        raise GrammarError("experiment_id must be a non-empty string")
    if not hypothesis_id or not hypothesis_id.strip():
        raise GrammarError("hypothesis_id must be a non-empty string")
    return {
        "generator": "strategy_mining.population_v1",
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "semantic_fingerprint": grammar.semantic_fingerprint(),
        "compatibility_status": grammar.compatibility_status(),
        "grammar": grammar.economic_fields(),
    }


def deterministic_trial_id(experiment_id: str, grammar: HypothesisGrammar) -> str:
    if not experiment_id or not experiment_id.strip():
        raise GrammarError("experiment_id must be a non-empty string")
    return f"mining:{experiment_id}:{grammar.semantic_fingerprint()[:16]}"
