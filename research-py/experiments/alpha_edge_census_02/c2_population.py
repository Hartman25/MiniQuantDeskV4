"""Census-02 frozen population authority: every Strategy trial coordinate and every semantic factor coordinate is a
deterministic, result-independent function of the grammar, the approved policy and the frozen protocol id."""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_borrow as bw  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_protocol as pr  # noqa: E402

SEED_UNIVERSE_FILE = bw.SEED_UNIVERSE_FILE


class PopulationRefusal(RuntimeError):
    pass


def _tiers_and_exclusion(decisions: dict) -> tuple[str, bool]:
    return decisions["grammar_tiers"], decisions["complement_handling"] == "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE"


def class_c_scope(decisions: dict) -> list[str]:
    """The frozen executable (Class-C) symbol list. Empty when the policy allows no executable shorts."""
    a = decisions.get("etf_borrow_assumption")
    return list(bw.validate_etf_assumption(a)["etf_short_scope"]) if a else []


def seed_symbols() -> list[str]:
    return sorted(bw.seed_universe_symbols())


def partitions_id() -> str:
    """Identity of the CURRENT partition-consumption truth (never the historical Census-01 object)."""
    return pr.partition_truth_id()


def strategy_universe_id(decisions: dict) -> str:
    """Pre-result identity of the executable scope: the seed-universe bytes plus the explicit Class-C list. Eligibility
    dispositions are post-acquisition facts and never enter it."""
    seed_sha = hashlib.sha256(SEED_UNIVERSE_FILE.read_bytes().replace(b"\r\n", b"\n")).hexdigest()
    return pr.sha256_canonical({"kind": "census02_strategy_scope", "seed_universe_sha256": seed_sha,
                                "class_c_scope": class_c_scope(decisions)})[:32]


def strategy_ids(decisions: dict, protocol_id: str) -> dict:
    return {"universe_id": strategy_universe_id(decisions), "partitions_id": partitions_id(), "protocol_id": protocol_id}


def strategy_cells(decisions: dict, protocol_id: str) -> list[tuple[int, dict, str, str]]:
    """[(config_index, config, symbol, trial_id)] in manifest order: config grammar order, then sorted Class-C symbol."""
    tiers, excl = _tiers_and_exclusion(decisions)
    configs = gr.build_configs(tiers, excl)
    ids = strategy_ids(decisions, protocol_id)
    cells = []
    for i, cfg in enumerate(configs):
        for sym in sorted(class_c_scope(decisions)):
            cells.append((i, cfg, sym, gr.trial_id(gr.trial_identity(cfg, sym, ids))))
    return cells


def sorted_root(items) -> str:
    h = hashlib.sha256()
    for t in sorted(items):
        h.update(str(t).encode("ascii") + b"\n")
    return h.hexdigest()


def strategy_population(decisions: dict, protocol_id: str) -> dict:
    cells = strategy_cells(decisions, protocol_id)
    tids = [c[3] for c in cells]
    if len(set(tids)) != len(tids):
        raise PopulationRefusal("duplicate Strategy trial coordinate")
    tiers, excl = _tiers_and_exclusion(decisions)
    configs = gr.build_configs(tiers, excl)
    return {"config_count": len(configs), "class_c_symbol_count": len(class_c_scope(decisions)),
            "class_c_scope": class_c_scope(decisions), "trial_count": len(tids), "population_root": sorted_root(tids),
            "manifest_order_root": hashlib.sha256("\n".join(tids).encode("ascii")).hexdigest(),
            "complement_tagged_configs": sum(1 for c in configs if gr.tags(c)["complement_of_census01"]),
            "complement_tagged_trials": sum(1 for c in cells if gr.tags(c[1])["complement_of_census01"]),
            "universe_id": strategy_universe_id(decisions), "partitions_id": partitions_id()}


def conditions(decisions: dict) -> list[dict]:
    tiers, excl = _tiers_and_exclusion(decisions)
    return gr.build_conditions(gr.build_configs(tiers, excl), excl)


def factor_coordinates(decisions: dict) -> list[tuple[str, int]]:
    return [(c["condition_id"], h) for c in conditions(decisions) for h in gr.HORIZONS]


def factor_population(decisions: dict) -> dict:
    """Semantic factor coordinates (condition x horizon). FactorSpec ids additionally bind the post-acquisition universe and
    provenance identities, so the specs are materialised only after authorised acquisition, ALL before any evaluation, and
    must match these coordinates exactly."""
    coords = factor_coordinates(decisions)
    if len(set(coords)) != len(coords):
        raise PopulationRefusal("duplicate factor coordinate")
    return {"condition_count": len(conditions(decisions)), "horizons": list(gr.HORIZONS), "factor_count": len(coords),
            "coordinate_root": sorted_root(f"{c}:{h}" for c, h in coords), "direction": pr.FACTOR_SEMANTICS["direction"],
            "scope": decisions["conditional_scope"], "scope_symbol_count": len(seed_symbols()),
            "family": pr.FACTOR_SEMANTICS["family"]}


def population_authority(decisions: dict, protocol_id: str) -> dict:
    return {"strategy_population": strategy_population(decisions, protocol_id), "factor_population": factor_population(decisions)}
