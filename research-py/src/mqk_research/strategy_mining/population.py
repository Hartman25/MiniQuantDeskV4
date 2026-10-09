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
import json
import hashlib
from dataclasses import dataclass, field
from typing import Any, Dict, List, Mapping, Sequence, Tuple

from mqk_research.strategy_mining.grammar import (
    AssetClassTag,
    Direction,
    GrammarError,
    HypothesisGrammar,
    MechanismFamily,
)


def _canonical_json(obj: Any) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), default=str)


def _sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


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

    def _param_combo_count(self, family: MechanismFamily) -> int:
        grid = self.parameter_grids.get(family, {})
        count = 1
        for values in grid.values():
            count *= max(len(values), 1)
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


def _declaration_id(decl: PopulationDeclaration) -> str:
    payload = {
        "mechanism_families": sorted(m.value for m in decl.mechanism_families),
        "directions": sorted(d.value for d in decl.directions),
        "asset_classes": sorted(a.value for a in decl.asset_classes),
        "parameter_grids": {
            family.value: {name: list(values) for name, values in grid.items()}
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
        "max_population_size": decl.max_population_size,
    }
    return _sha256_hex(_canonical_json(payload))


def generate_population(decl: PopulationDeclaration) -> PopulationManifest:
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
    """
    return {
        "generator": "strategy_mining.population_v1",
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "semantic_fingerprint": grammar.semantic_fingerprint(),
        "compatibility_status": grammar.compatibility_status(),
        "grammar": grammar.economic_fields(),
    }


def deterministic_trial_id(experiment_id: str, grammar: HypothesisGrammar) -> str:
    return f"mining:{experiment_id}:{grammar.semantic_fingerprint()[:16]}"
