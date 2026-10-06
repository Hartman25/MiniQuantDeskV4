"""Corrected Alpha Edge Census driver. Artifacts go under runs/alpha_edge_census_01_corrected/ (the rejected
execution's runs/alpha_edge_census_01/ is never read or written).

Stages (each idempotent; frozen manifests are immutable once written):
  acquire        per-symbol SIP/adjustment=all bars for [2016-01-01, 2024-01-01) -> typed ELIGIBLE/EXCLUDED_* dispositions
                 -> eligible-universe + search-space manifests
  bars-manifest  immutable census bars/provenance manifest over the eligible universe
  freeze         registers the whole StrategyEdge trial population and ConditionalEdge FactorSpec population
                 (attempts must be 0) and writes the POPULATION_FREEZE_PROOF
  run-strategy   resumable StrategyEdge Pass-1 execution (frozen-population gate first)
  run-factors    resumable registered ConditionalEdge factor evaluations (one durable attempt per evaluation)
  registry       complete-family FDR + qualification ledgers + Edge Registry V2 + campaign evidence
"""

from __future__ import annotations

import argparse
import hashlib
import json
import multiprocessing as mp
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import data as dt  # noqa: E402
import edge_registry as er  # noqa: E402
import partitions as pt  # noqa: E402
import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

RUN_DIR = HERE.parents[1] / "runs" / "alpha_edge_census_01_corrected"
DATA_DIR = RUN_DIR / "data"
REGISTRY_DB = RUN_DIR / "registry.sqlite"  # StrategyEdge registry (accepted; never written by the V3 factor run)
FACTOR_REGISTRY_DB = RUN_DIR / "registry_conditional_v3.sqlite"
BARS_MANIFEST = HERE / "ALPHA_CENSUS_BARS_MANIFEST_V2.json"
FREEZE_PROOF = HERE / "POPULATION_FREEZE_PROOF_V2.json"
CAMPAIGN_EVIDENCE = HERE / "CAMPAIGN_EVIDENCE_V3.json"
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
    freeze = ce.register_population(store, space, cells)
    gate = ce.require_frozen_population(store, space, cells, allow_attempts=False)
    per_family = {f: d["configs"] for f, d in grammar["family_templates"].items()}
    proof = {
        "schema_version": "alpha_census_population_freeze_proof_v2", "experiment_id": ss.EXPERIMENT_ID,
        "strategy_edge_config_count": len(grammar["configs"]), "config_count_required": ss.EXPECTED_CONFIG_COUNT,
        "family_set": sorted(per_family), "per_family_config_counts": per_family,
        "seed_symbol_count": uni["seed_symbol_count"], "eligible_symbol_count": uni["symbol_count"],
        "excluded_symbol_count": len(uni["excluded"]),
        "disposition_counts": _counts(uni["dispositions"]),
        "derived_strategy_edge_trial_count": len(cells),
        "registered_strategy_trials": gate["registered_trials"], "strategy_attempts": gate["strategy_attempts"],
        "attempts_at_freeze": gate["strategy_attempts"],
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
                                            "derived_strategy_edge_trial_count", "attempts_at_freeze",
                                            "max_economic_input_end_ts")}, sort_keys=True))

FACTOR_DIR = RUN_DIR / "factor_eval"
FACTOR_REC_DIR = FACTOR_DIR / "records"
FACTOR_ORIGIN = "alpha_census_factor_pass1"
FACTOR_WORKERS = 6
_W: dict = {}


def _run_context():
    uni, part, prot, space, grammar, bm, ctx, cells = _setup()
    ce.require_frozen_population(ResearchResultStore(REGISTRY_DB), space, cells, allow_attempts=True)
    return uni, part, prot, space, bm, ctx, cells


def _load_universe(uni: dict, bm: dict):
    return sg.Universe(ce.load_bars(uni, DATA_DIR, bm))


def cmd_run_strategy(a) -> None:
    uni, _part, prot, space, bm, ctx, cells = _run_context()
    t0 = time.time()
    U = _load_universe(uni, bm)
    meta = ce.symbol_meta(bm, prot)
    store = ResearchResultStore(REGISTRY_DB)
    res = ce.run_chunks(store, U, space, cells, meta, RUN_DIR, chunk_size=prot["chunking"]["cells_per_chunk"],
                        max_chunks=a.max_chunks, log=lambda m: print(f"{time.time() - t0:7.0f}s {m}", flush=True))
    print(json.dumps(res, sort_keys=True))


def _rec_path(ci: int) -> Path:
    return FACTOR_REC_DIR / f"config_{ci:04d}.jsonl"


def _config_records(ci: int, conditions: list[dict], ctx: dict) -> list[dict] | None:
    """The condition's five evaluation records iff the file is complete and bound to the registered factor ids."""
    p = _rec_path(ci)
    if not p.exists():
        return None
    rows = [json.loads(x) for x in p.read_text(encoding="utf-8").splitlines() if x.strip()]
    want = [cd.factor_spec(conditions[ci], h, ctx).compute_factor_id() for h in ss.CONDITIONAL_HORIZONS]
    terminal = {cd.EVAL_STATUS_SUCCEEDED, "not_evaluable"}
    ok = [r["factor_id"] for r in rows] == want and all(r["status"] in terminal for r in rows)
    return rows if ok else None


def _init_worker(uni: dict, bm: dict, ctx: dict) -> None:
    _W["U"], _W["ctx"], _W["cache"] = _load_universe(uni, bm), ctx, cd.PermutationCache()


def _eval_config(item: tuple[int, dict]) -> tuple[int, str]:
    ci, config = item
    ctx, store = _W["ctx"], ResearchResultStore(FACTOR_REGISTRY_DB)
    recs = []
    try:
        for h in ss.CONDITIONAL_HORIZONS:
            fid = cd.factor_spec(config, h, ctx).compute_factor_id()
            for att in store.list_factor_evaluation_attempts(fid):
                if att["status"] == "started":
                    store.finalize_factor_evaluation_attempt(
                        att["attempt_id"], status="failed", expected_factor_id=fid,
                        expected_evaluation_id=att["evaluation_id"], failure_reason="infrastructure_interrupted")
            recs.append(cd.evaluate_factor(FACTOR_REGISTRY_DB, FACTOR_DIR / "artifacts", _W["U"], config, h, ctx,
                                           origin=FACTOR_ORIGIN, cache=_W["cache"], metadata={"config_index": ci}))
    except Exception as exc:  # noqa: BLE001 - infrastructure fault: attempt is durably failed, config retried on resume
        return ci, f"{type(exc).__name__}: {str(exc)[:200]}"
    tmp = _rec_path(ci).with_suffix(".tmp")
    body = "\n".join(json.dumps(r, sort_keys=True, separators=(",", ":")) for r in recs) + "\n"
    tmp.write_bytes(body.encode("utf-8"))
    os.replace(tmp, _rec_path(ci))
    return ci, "ok"


def cmd_run_factors(a) -> None:
    uni, _part, _prot, _space, bm, ctx, cells = _run_context()
    configs = ce.cell_conditions(cells)
    FACTOR_REC_DIR.mkdir(parents=True, exist_ok=True)
    pending = [(ci, c) for ci, c in enumerate(configs) if _config_records(ci, configs, ctx) is None]
    if a.max_configs is not None:
        pending = pending[:a.max_configs]
    print(f"configs total={len(configs)} pending={len(pending)}", flush=True)
    t0, failed = time.time(), []
    with mp.get_context("spawn").Pool(a.workers, initializer=_init_worker, initargs=(uni, bm, ctx)) as pool:
        for n, (ci, status) in enumerate(pool.imap_unordered(_eval_config, pending), 1):
            if status != "ok":
                failed.append((ci, status))
            if n % 10 == 0 or status != "ok":
                print(f"{time.time() - t0:7.0f}s {n}/{len(pending)} config {ci} {status}", flush=True)
    print(json.dumps({"evaluated": len(pending) - len(failed), "failed": len(failed)}))
    if failed:
        raise SystemExit(f"fail-closed: {len(failed)} configs hit infrastructure faults; re-run run-factors to retry: {failed[:3]}")


def cmd_registry(_a) -> None:
    uni, _part, prot, space, bm, ctx, cells = _run_context()
    configs = ce.cell_conditions(cells)
    records = []
    for ci in range(len(configs)):
        rows = _config_records(ci, configs, ctx)
        if rows is None:
            raise ce.GateRefusal(f"config {ci} has no complete registered factor evaluation records; run run-factors")
        records += rows
    store = ResearchResultStore(REGISTRY_DB)
    digest = store.trial_attempt_digest(ss.EXPERIMENT_ID)
    if any(d["succeeded"] < 1 or d["started"] for d in digest.values()):
        raise ce.GateRefusal("StrategyEdge population is not fully terminal; run run-strategy")
    fdr = cd.family_fdr_report(FACTOR_REGISTRY_DB, records)
    _dump(RUN_DIR / "factor_fdr_report_v3.json", fdr)
    meta = ce.symbol_meta(bm, prot)
    summary = er.build_registry(space, uni, cells, meta, prot, RUN_DIR, ctx=ctx, factor_records=records, fdr=fdr)
    fstore = ResearchResultStore(FACTOR_REGISTRY_DB)
    fattempts = [a for fid in cd.expected_factor_ids(configs, ctx) for a in fstore.list_factor_evaluation_attempts(fid)]
    outs = (er.OUT_SEARCH_LEDGER, er.OUT_FACTOR_LEDGER, er.OUT_EDGES, er.OUT_SUMMARY, "factor_fdr_report_v3.json")
    _dump(CAMPAIGN_EVIDENCE, {
        "schema_version": "alpha_census_campaign_evidence_v3", "experiment_id": ss.EXPERIMENT_ID,
        "eligible_symbol_count": uni["symbol_count"], "strategy_edge_config_count": len(ce.cell_configs(cells)),
        "semantic_condition_count": len(configs),
        "strategy_edge_trial_count": len(cells), "registered_factor_count": len(records),
        "strategy_attempts": sum(d["attempts"] for d in digest.values()),
        "strategy_attempts_failed": sum(d["attempts"] - d["succeeded"] for d in digest.values()),
        "factor_attempts": len(fattempts), "factor_attempts_failed": sum(a["status"] == "failed" for a in fattempts),
        "factor_statuses": summary["factor_statuses"], "strategy_edges": summary["strategy_edges"],
        "conditional_edges": summary["conditional_edges"], "fdr_status": fdr["status"], "fdr_alpha": fdr["alpha"],
        "fdr_declared_population": fdr["declared_population_count"], "judge_status": summary["JUDGE_STATUS"],
        "strategy_positive_below_floor": summary["strategy_positive_below_floor"],
        "conditional_positive_below_floor": summary["conditional_positive_below_floor"],
        "bars_manifest_sha256": bm["manifest_sha256"], "output_sha256": {n: _sha(RUN_DIR / n) for n in outs},
        "validation_status": "NOT_VALIDATED", "promotion_authority": "NONE", "confirmation": "NOT_RUN"})
    print("fdr:", fdr["status"], "registry:", json.dumps({k: summary[k] for k in summary if not isinstance(summary[k], (dict, list))},
                                                          sort_keys=True))


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
    r = sub.add_parser("run-strategy")
    r.add_argument("--max-chunks", type=int, default=None)
    r.set_defaults(fn=cmd_run_strategy)
    r = sub.add_parser("run-factors")
    r.add_argument("--max-configs", type=int, default=None)
    r.add_argument("--workers", type=int, default=FACTOR_WORKERS)
    r.set_defaults(fn=cmd_run_factors)
    sub.add_parser("registry").set_defaults(fn=cmd_registry)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
