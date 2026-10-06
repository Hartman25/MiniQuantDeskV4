"""Alpha Edge Census Pass 1 driver. Stages: bars-manifest -> freeze -> run. Artifacts go under runs/alpha_edge_census_01/.

  python experiments/alpha_edge_census_01/run_census.py bars-manifest   # writes the immutable bars manifest
  python experiments/alpha_edge_census_01/run_census.py freeze          # registers the population (attempts must be 0)
  python experiments/alpha_edge_census_01/run_census.py run [--max-chunks N]   # resumable execution + registry
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import census as ce  # noqa: E402
import edge_registry as er  # noqa: E402
import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

RUN_DIR = HERE.parents[1] / "runs" / "alpha_edge_census_01"
DATA_DIR = RUN_DIR / "data"
BARS_MANIFEST = HERE / "ALPHA_CENSUS_BARS_MANIFEST_V1.json"
POPULATION = HERE / "ALPHA_CENSUS_POPULATION_V1.json"
FROZEN = ("UNIVERSE", "PARTITIONS", "PROTOCOL", "SEARCH_SPACE")


def _load(name: str) -> dict:
    return json.loads((HERE / f"ALPHA_CENSUS_{name}_V1.json").read_text(encoding="utf-8"))


def _frozen() -> tuple[dict, dict, dict, dict]:
    uni, part, prot, space = (_load(n) for n in FROZEN)
    if part != ss.build_partitions() or prot != ss.build_protocol() or ss.build_search_space(uni, part, prot) != space:
        raise ce.GateRefusal("frozen manifests differ from regenerated authority")
    return uni, part, prot, space


def _dump(path: Path, obj: dict) -> None:
    path.write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def cmd_bars_manifest(_a) -> None:
    uni, _p, _q, _s = _frozen()
    doc = ce.build_bars_manifest(uni, DATA_DIR)
    if BARS_MANIFEST.exists():
        if json.loads(BARS_MANIFEST.read_text(encoding="utf-8")) != doc:
            raise ce.GateRefusal("bars manifest is immutable and differs from the data on disk")
        print("bars manifest already frozen and identical")
        return
    _dump(BARS_MANIFEST, doc)
    disp: dict = {}
    for r in doc["symbols"].values():
        disp[r["disposition"]] = disp.get(r["disposition"], 0) + 1
    print("bars manifest written", doc["manifest_sha256"], disp)


def _setup():
    uni, _part, prot, space = _frozen()
    bm = json.loads(BARS_MANIFEST.read_text(encoding="utf-8"))
    configs, symbols, _ids, cells = ce.population(uni, space)
    if ss.population_root(configs, symbols, {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}) != \
            space["population_root_sha256"]:
        raise ce.GateRefusal("population root differs from the frozen search space")
    if len(cells) != space["strategy_cell_count"]:
        raise ce.GateRefusal("expanded population size differs from the frozen search space")
    return uni, prot, space, bm, cells


def cmd_freeze(_a) -> None:
    _uni, _prot, space, _bm, cells = _setup()
    store = ResearchResultStore(RUN_DIR / "registry.sqlite")
    freeze = ce.register_population(store, space, cells)
    gate = ce.require_frozen_population(store, space, cells, allow_attempts=False)
    doc = {"schema_version": "alpha_census_population_v1", "freeze": freeze, "gate": gate,
           "registered_equals_expected": True, "attempts_at_freeze": gate["attempts"]}
    if POPULATION.exists():
        if json.loads(POPULATION.read_text(encoding="utf-8")) != doc:
            raise ce.GateRefusal("population record is immutable and differs")
    else:
        _dump(POPULATION, doc)
    print("population frozen:", json.dumps(gate, sort_keys=True))


def cmd_run(a) -> None:
    uni, prot, space, bm, cells = _setup()
    store = ResearchResultStore(RUN_DIR / "registry.sqlite")
    ce.require_frozen_population(store, space, cells, allow_attempts=True)
    t0 = time.time()
    bars = ce.load_bars(uni, DATA_DIR, bm)
    meta = ce.symbol_meta(bm, prot)
    U = sg.Universe(bars)
    print(f"loaded {len(bars)} symbols / universe built in {time.time() - t0:.0f}s", flush=True)
    res = ce.run_chunks(store, U, space, cells, meta, RUN_DIR, chunk_size=prot["chunking"]["cells_per_chunk"],
                        max_chunks=a.max_chunks, log=lambda m: print(f"{time.time() - t0:7.0f}s {m}", flush=True))
    print(json.dumps(res))
    if res["chunks_run"] + res["chunks_skipped_terminal"] == res["chunks_total"]:
        summary = er.build_registry(space, uni, cells, meta, prot, RUN_DIR)
        print("registry:", json.dumps({k: summary[k] for k in ("cells_total", "strategy_edges", "conditional_edges")}))
        _write_evidence(summary)


def _sha(p: Path) -> str:
    import hashlib
    return hashlib.sha256(p.read_bytes()).hexdigest()


def _write_evidence(summary: dict) -> None:
    files = {f"ALPHA_CENSUS_{n}_V1.json": HERE / f"ALPHA_CENSUS_{n}_V1.json" for n in FROZEN}
    files[BARS_MANIFEST.name] = BARS_MANIFEST
    files[POPULATION.name] = POPULATION
    for n in ("search_ledger_v1.jsonl", "edge_registry_v1.jsonl", "edge_registry_summary_v1.json"):
        files[n] = RUN_DIR / n
    ev = {"schema_version": "alpha_census_campaign_evidence_v1", "sha256": {k: _sha(v) for k, v in sorted(files.items())},
          "summary_counts": {k: summary[k] for k in ("cells_total", "strategy_edges", "conditional_edges")}}
    _dump(RUN_DIR / "campaign_evidence_v1.json", ev)
    print("campaign evidence written")


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("bars-manifest").set_defaults(fn=cmd_bars_manifest)
    sub.add_parser("freeze").set_defaults(fn=cmd_freeze)
    r = sub.add_parser("run")
    r.add_argument("--max-chunks", type=int, default=None)
    r.set_defaults(fn=cmd_run)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
