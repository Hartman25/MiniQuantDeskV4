"""Corrected Alpha Edge Census driver. Artifacts go under runs/alpha_edge_census_01_corrected/ (the rejected
execution's runs/alpha_edge_census_01/ is never read or written).

Stages (each idempotent; frozen manifests are immutable once written):
  acquire        per-symbol SIP/adjustment=all bars for [2016-01-01, 2024-01-01) -> typed ELIGIBLE/EXCLUDED_* dispositions
                 -> eligible-universe + search-space manifests
  bars-manifest  immutable census bars/provenance manifest over the eligible universe
  freeze         registers the whole StrategyEdge trial population and ConditionalEdge FactorSpec population
                 (attempts must be 0) and writes the POPULATION_FREEZE_PROOF
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import data as dt  # noqa: E402
import partitions as pt  # noqa: E402
import search_space as ss  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

RUN_DIR = HERE.parents[1] / "runs" / "alpha_edge_census_01_corrected"
DATA_DIR = RUN_DIR / "data"
REGISTRY_DB = RUN_DIR / "registry.sqlite"
BARS_MANIFEST = HERE / "ALPHA_CENSUS_BARS_MANIFEST_V2.json"
FREEZE_PROOF = HERE / "POPULATION_FREEZE_PROOF_V2.json"
_MANIFESTS = (ss.SEED_UNIVERSE_FILE, ss.GRAMMAR_FILE, ss.PARTITIONS_FILE, ss.PROTOCOL_FILE, ss.UNIVERSE_FILE,
              ss.SEARCH_SPACE_FILE, BARS_MANIFEST)


def _load(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _dump(path: Path, obj: dict) -> None:
    path.write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def _sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def _write_immutable(path: Path, doc: dict, what: str) -> None:
    if path.exists():
        if _load(path) != doc:
            raise ce.GateRefusal(f"{what} is immutable and differs from the regenerated authority")
        return
    _dump(path, doc)


def cmd_acquire(_a) -> None:
    seed = _load(ss.SEED_UNIVERSE_FILE)
    if seed != ss.build_seed_universe():
        raise ce.GateRefusal("seed universe differs from the live registry snapshot; fail closed")
    statuses = dt.acquire_universe(seed["symbols"], DATA_DIR)
    dispositions = {s: dt.classify_eligibility(s, statuses[s], DATA_DIR / s) for s in seed["symbols"]}
    ss.validate_dispositions(seed, dispositions)
    expected = ss.build_universe(seed, dispositions)
    if ss.UNIVERSE_FILE.exists() and _load(ss.UNIVERSE_FILE) != expected:
        raise ce.GateRefusal("eligible universe is immutable and differs from the data on disk")
    space = ss.write_population_manifests(dispositions)
    counts: dict = {}
    for r in dispositions.values():
        counts[r["disposition"]] = counts.get(r["disposition"], 0) + 1
    print("dispositions:", json.dumps(counts, sort_keys=True))
    print("eligible:", expected["symbol_count"], "strategy_trial_count:", space["strategy_trial_count"])


def _frozen() -> tuple[dict, dict, dict, dict, dict]:
    seed, grammar, part, prot = (_load(p) for p in (ss.SEED_UNIVERSE_FILE, ss.GRAMMAR_FILE, ss.PARTITIONS_FILE,
                                                    ss.PROTOCOL_FILE))
    uni, space = _load(ss.UNIVERSE_FILE), _load(ss.SEARCH_SPACE_FILE)
    if grammar != ss.build_grammar() or part != ss.build_partitions() or prot != ss.build_protocol():
        raise ce.GateRefusal("frozen manifests differ from regenerated authority")
    if ss.build_universe(seed, uni["dispositions"]) != uni or ss.build_search_space(grammar, uni, part, prot) != space:
        raise ce.GateRefusal("universe/search space differ from the frozen dispositions")
    return uni, part, prot, space, grammar


def cmd_bars_manifest(_a) -> None:
    uni, _p, _q, _s, _g = _frozen()
    doc = ce.build_bars_manifest(uni, DATA_DIR)
    _write_immutable(BARS_MANIFEST, doc, "bars manifest")
    print("bars manifest", doc["manifest_sha256"])


def _setup():
    uni, part, prot, space, grammar = _frozen()
    ss.assert_grammar_authority(grammar["configs"])
    bm = _load(BARS_MANIFEST)
    if bm["universe_id"] != ss.sha256_canonical(uni)[:32]:
        raise ce.GateRefusal("bars manifest is bound to a different universe")
    ctx = cd.population_context(uni, prot, part, bm)
    configs, symbols, ids, cells = ce.population(uni, space)
    if ss.population_root(configs, symbols, ids) != space["population_root_sha256"]:
        raise ce.GateRefusal("population root differs from the frozen search space")
    if len(cells) != space["strategy_trial_count"] or len(cells) != ss.EXPECTED_CONFIG_COUNT * len(symbols):
        raise ce.GateRefusal("expanded population size differs from 434 x eligible symbols")
    return uni, part, prot, space, grammar, bm, ctx, cells


def _max_input_ts(bm: dict) -> str:
    return max(r["last_end_ts"] for r in bm["symbols"].values() if r["disposition"] == ce.DATA_PRESENT)


def cmd_freeze(_a) -> None:
    uni, part, prot, space, grammar, bm, ctx, cells = _setup()
    max_ts = _max_input_ts(bm)
    pt.require_discovery_only([max_ts], what="census bars manifest max timestamp")
    store = ResearchResultStore(REGISTRY_DB)
    freeze = ce.register_population(store, space, cells, ctx)
    gate = ce.require_frozen_population(store, space, cells, ctx, allow_attempts=False)
    per_family = {f: d["configs"] for f, d in grammar["family_templates"].items()}
    proof = {
        "schema_version": "alpha_census_population_freeze_proof_v2", "experiment_id": ss.EXPERIMENT_ID,
        "strategy_edge_config_count": len(grammar["configs"]), "config_count_required": ss.EXPECTED_CONFIG_COUNT,
        "family_set": sorted(per_family), "per_family_config_counts": per_family,
        "seed_symbol_count": uni["seed_symbol_count"], "eligible_symbol_count": uni["symbol_count"],
        "excluded_symbol_count": len(uni["excluded"]),
        "disposition_counts": _counts(uni["dispositions"]),
        "derived_strategy_edge_trial_count": len(cells),
        "conditional_horizons": list(ss.CONDITIONAL_HORIZONS), "conditional_factor_count": gate["registered_factors"],
        "registered_strategy_trials": gate["registered_trials"], "registered_factors": gate["registered_factors"],
        "strategy_attempts": gate["strategy_attempts"], "factor_evaluation_attempts": gate["factor_attempts"],
        "attempts_at_freeze": gate["strategy_attempts"] + gate["factor_attempts"],
        "population_hashes": freeze,
        "partition_ids": {"partitions_id": space["partitions_id"], "partitions_sha256": ss.sha256_canonical(part)},
        "universe_id": space["universe_id"], "protocol_id": space["protocol_id"], "search_space_id": space["search_space_id"],
        "grammar_id": grammar["grammar_id"], "bars_manifest_sha256": bm["manifest_sha256"],
        "max_economic_input_end_ts": max_ts, "discovery_end_exclusive": str(pt.DISCOVERY_END_EXCLUSIVE),
        "manifest_file_sha256": {p.name: _sha(p) for p in _MANIFESTS},
    }
    if proof["attempts_at_freeze"] != 0:
        raise ce.GateRefusal("attempts exist at freeze time")
    if proof["derived_strategy_edge_trial_count"] != ss.EXPECTED_CONFIG_COUNT * proof["eligible_symbol_count"]:
        raise ce.GateRefusal("trial count is not 434 x eligible symbols")
    _write_immutable(FREEZE_PROOF, proof, "population freeze proof")
    print(json.dumps({k: proof[k] for k in ("strategy_edge_config_count", "eligible_symbol_count",
                                            "derived_strategy_edge_trial_count", "conditional_factor_count",
                                            "attempts_at_freeze", "max_economic_input_end_ts")}, sort_keys=True))


def _counts(dispositions: dict) -> dict:
    out: dict = {}
    for r in dispositions.values():
        out[r["disposition"]] = out.get(r["disposition"], 0) + 1
    return dict(sorted(out.items()))


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("acquire").set_defaults(fn=cmd_acquire)
    sub.add_parser("bars-manifest").set_defaults(fn=cmd_bars_manifest)
    sub.add_parser("freeze").set_defaults(fn=cmd_freeze)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
