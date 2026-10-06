"""Corrected Alpha Edge Census driver. Artifacts go under runs/alpha_edge_census_01_corrected/ (the rejected
execution's runs/alpha_edge_census_01/ is never read or written).

Stages (each idempotent; frozen manifests are immutable once written):
  acquire        per-symbol SIP/adjustment=all bars for [2016-01-01, 2024-01-01) -> typed ELIGIBLE/EXCLUDED_* dispositions
                 -> eligible-universe + search-space manifests
  bars-manifest  immutable census bars/provenance manifest over the eligible universe
  freeze         registers the whole StrategyEdge trial population (attempts must be 0) and writes the
                 POPULATION_FREEZE_PROOF (V2; historical, immutable)
  freeze-factors V3 factor-only freeze: semantic condition grammar, V2 rejection record, 1,095 FactorSpecs in the V3
                 registry bound to the accepted Strategy state; must be committed before V3 factor attempt #1
  run-strategy   resumable StrategyEdge Pass-1 execution (frozen-population gate first)
  run-factors    resumable registered ConditionalEdge factor evaluations (factor-level resume; a terminal factor is never re-attempted)
  registry       complete-family V3 FDR + qualification ledgers + Edge Registry V3 + campaign evidence
"""

from __future__ import annotations

import argparse
import hashlib
import json
import multiprocessing as mp
import sqlite3
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
CAMPAIGN_EVIDENCE_V2 = HERE / "CAMPAIGN_EVIDENCE_V2.json"
FACTOR_FREEZE_PROOF = HERE / "FACTOR_FREEZE_PROOF_V3.json"
V2_DISPOSITION_FILE = HERE / "CONDITIONAL_V2_DISPOSITION.json"
V2_LEDGER = RUN_DIR / "search_ledger_v2.jsonl"
ACCEPTED_STRATEGY_EDGES = {"DISCOVERED_WEAK": 2851, "DISCOVERED_MODERATE": 789, "DISCOVERED_STRONG": 0}
_MANIFESTS = (ss.SEED_UNIVERSE_FILE, ss.GRAMMAR_FILE, ss.PARTITIONS_FILE, ss.PROTOCOL_FILE, ss.UNIVERSE_FILE,
              ss.SEARCH_SPACE_FILE, BARS_MANIFEST)
_V3_MANIFESTS = (*_MANIFESTS, ss.CONDITION_GRAMMAR_FILE, V2_DISPOSITION_FILE)


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


def _norm_sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def _factor_setup():
    """Everything the V3 factor freeze binds, recomputed from authority: the semantic conditions, the grammar file, and the
    accepted Strategy state (population, terminal attempts, raw chunks, accepted search ledger)."""
    uni, part, prot, space, grammar, bm, ctx, cells = _setup()
    configs, conditions = ce.cell_configs(cells), ce.cell_conditions(cells)
    ss.assert_condition_authority(conditions, configs)
    cg = ss.build_condition_grammar(configs)
    if ss.CONDITION_GRAMMAR_FILE.exists() and _load(ss.CONDITION_GRAMMAR_FILE) != cg:
        raise ce.GateRefusal("condition grammar file is immutable and differs from the regenerated authority")
    ev2 = _load(CAMPAIGN_EVIDENCE_V2)
    binding = ce.strategy_binding(ResearchResultStore(REGISTRY_DB), space, cells, RUN_DIR,
                                  chunk_size=prot["chunking"]["cells_per_chunk"], ledger_path=V2_LEDGER,
                                  expected_ledger_sha256=ev2["output_sha256"]["search_ledger_v2.jsonl"])
    freeze = cd.factor_freeze_record(conditions, ctx, cg["condition_grammar_id"], binding)
    return uni, part, prot, space, grammar, bm, ctx, cells, conditions, cg, ev2, freeze


def _factor_context():
    uni, _part, prot, space, _grammar, bm, ctx, cells, conditions, _cg, _ev2, freeze = _factor_setup()
    cd.require_frozen_factor_population(FACTOR_REGISTRY_DB, conditions, ctx, freeze, allow_attempts=True)
    return uni, prot, space, bm, ctx, cells, conditions


def _v2_disposition(configs: list[dict], ctx: dict, ev2: dict) -> dict:
    """Preserve-and-relabel record of the rejected V2 conditional population. Read-only over the V2 evidence."""
    con = sqlite3.connect(f"file:{REGISTRY_DB.as_posix()}?mode=ro", uri=True)
    try:
        v2_reg = sorted(r[0] for r in con.execute("select factor_id from research_factors where family=?",
                                                  (cd.FACTOR_FAMILY_V2,)))
        att = dict(con.execute("select status, count(*) from research_factor_evaluation_attempts where factor_id in "
                               "(select factor_id from research_factors where family=?) group by status",
                               (cd.FACTOR_FAMILY_V2,)).fetchall())
    finally:
        con.close()
    expected = sorted(cd.v2_factor_spec(c, h, ctx).compute_factor_id() for c in configs for h in ss.CONDITIONAL_HORIZONS)
    if v2_reg != expected or sum(att.values()) != ev2["factor_attempts"]:
        raise ce.GateRefusal("V2 conditional evidence differs from the recorded V2 population/attempts")
    names = (er.OUT_SEARCH_LEDGER.replace("v3", "v2"), er.OUT_FACTOR_LEDGER.replace("v3", "v2"),
             er.OUT_EDGES.replace("v3", "v2"), er.OUT_SUMMARY.replace("v3", "v2"), "factor_fdr_report_v2.json")
    return {"schema_version": "alpha_census_conditional_v2_disposition", "experiment_id": ss.EXPERIMENT_ID,
            "disposition": ss.V2_CONDITIONAL_DISPOSITION, "authoritative": False,
            "reason": "V2 identity carried the full Strategy configuration (incl. exit/hold execution state) into the "
                      "FactorSpec, minting duplicate hypotheses for identical conditional relationships (434 configs x 5 "
                      "horizons = 2,170 vs 219 semantic conditions x 5 = 1,095)",
            "v2_factor_family": cd.FACTOR_FAMILY_V2, "v2_factor_count": len(v2_reg),
            "v2_population_root": ss.sha256_canonical(v2_reg), "v2_attempts_by_status": dict(sorted(att.items())),
            "v2_conditional_edges": ev2["conditional_edges"], "v2_fdr_status": ev2["fdr_status"],
            "v2_fdr_declared_population": ev2["fdr_declared_population"],
            "evidence_sha256": {n: ce.sha256_file(RUN_DIR / n) for n in names},
            "campaign_evidence_v2_normalized_sha256": _norm_sha(CAMPAIGN_EVIDENCE_V2),
            "evidence_preserved_unmodified": True, "v2_results_used_to_choose_v3_parameters": False,
            "v3_population_disjoint_from_v2": True}


def cmd_freeze_factors(_a) -> None:
    uni, part, prot, space, grammar, bm, ctx, cells, conditions, cg, ev2, freeze = _factor_setup()
    max_ts = _max_input_ts(bm)
    pt.require_discovery_only([max_ts], what="census bars manifest max timestamp")
    if ev2["strategy_edges"] != ACCEPTED_STRATEGY_EDGES:
        raise ce.GateRefusal("accepted StrategyEdge classification counts differ from the verified result")
    reg_before = ce.sha256_file(REGISTRY_DB)
    configs = ce.cell_configs(cells)
    _write_immutable(ss.CONDITION_GRAMMAR_FILE, cg, "condition grammar")
    disp = _v2_disposition(configs, ctx, ev2)
    _write_immutable(V2_DISPOSITION_FILE, disp, "V2 conditional disposition")
    equivalence = cd.condition_equivalence_proof(_load_universe(uni, bm), configs, conditions)
    ids = cd.register_factor_population(FACTOR_REGISTRY_DB, conditions, ctx, freeze)
    gate = cd.require_frozen_factor_population(FACTOR_REGISTRY_DB, conditions, ctx, freeze, allow_attempts=False)
    if gate["attempts"] or gate["registered"] != ss.EXPECTED_CONDITIONAL_FACTOR_COUNT or len(ids) != gate["registered"]:
        raise ce.GateRefusal("V3 factor freeze requires 1,095 registered factors and zero attempts")
    if ce.sha256_file(REGISTRY_DB) != reg_before:
        raise ce.GateRefusal("the accepted StrategyEdge registry changed during the factor freeze")
    proof = {
        "schema_version": "alpha_census_factor_freeze_proof_v3", "experiment_id": ss.EXPERIMENT_ID,
        "mission": "V4-ALPHA-EDGE-CENSUS-01-FINAL-CONDITIONAL-CORRECTION-02", "scope": "FACTOR_ONLY",
        "strategy_edge_config_count": len(configs), "strategy_trial_count": len(cells),
        "strategy_binding": freeze["strategy_binding"], "strategy_edges_accepted": ev2["strategy_edges"],
        "strategy_attempts_created_by_this_mission": 0, "strategy_registry_sqlite_sha256_before_after": reg_before,
        "semantic_condition_count": len(conditions), "per_family_condition_counts": cg["per_family_condition_counts"],
        "condition_grammar_id": cg["condition_grammar_id"], "condition_source_mapping_sha256": cg["source_mapping_sha256"],
        "conditional_horizons": list(ss.CONDITIONAL_HORIZONS), "registered_factors": gate["registered"],
        "conditional_factor_count": freeze["conditional_factor_count"], "factor_evaluation_attempts": gate["attempts"],
        "conditional_factor_population_root": freeze["conditional_factor_population_root"],
        "factor_family": cd.FACTOR_FAMILY, "v2_conditional_disposition": disp["disposition"],
        "v2_factor_count": disp["v2_factor_count"], "v2_population_root": disp["v2_population_root"],
        "condition_equivalence_real_data": equivalence, "universe_identity": ctx["universe_identity"],
        "data_provenance_identity": ctx["data_provenance_identity"], "bars_manifest_sha256": bm["manifest_sha256"],
        "partition_ids": {"partitions_id": space["partitions_id"], "partitions_sha256": ss.sha256_canonical(part)},
        "universe_id": space["universe_id"], "protocol_id": space["protocol_id"], "search_space_id": space["search_space_id"],
        "grammar_id": grammar["grammar_id"], "max_economic_input_end_ts": max_ts,
        "discovery_end_exclusive": str(pt.DISCOVERY_END_EXCLUSIVE),
        "manifest_file_sha256": {pth.name: _sha(pth) for pth in _V3_MANIFESTS}}
    _write_immutable(FACTOR_FREEZE_PROOF, proof, "V3 factor freeze proof")
    print(json.dumps({k: proof[k] for k in ("semantic_condition_count", "registered_factors", "factor_evaluation_attempts",
                                            "strategy_attempts_created_by_this_mission", "v2_conditional_disposition")},
                     sort_keys=True))
    print("equivalence:", json.dumps({k: v for k, v in equivalence.items()}, sort_keys=True))


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


def _init_worker(uni: dict, bm: dict, ctx: dict) -> None:
    _W["U"], _W["ctx"], _W["cache"] = _load_universe(uni, bm), ctx, cd.PermutationCache()


def _eval_condition(item: tuple[int, dict]) -> tuple[int, str, dict]:
    """Resolve the five factors of one condition at factor granularity: a terminal factor is reused, never re-attempted;
    each horizon is persisted as soon as it is terminal, so a later horizon failing cannot cause a rerun."""
    ci, cond = item
    actions: dict = {}
    try:
        for h in ss.CONDITIONAL_HORIZONS:
            _rec, action = cd.resolve_factor(FACTOR_REGISTRY_DB, FACTOR_DIR / "artifacts", FACTOR_REC_DIR, _W["U"], cond, h,
                                             _W["ctx"], origin=FACTOR_ORIGIN, cache=_W["cache"],
                                             metadata={"condition_index": ci})
            actions[action] = actions.get(action, 0) + 1
    except Exception as exc:  # noqa: BLE001 - the attempt is durably failed (or refused); remaining horizons resume later
        return ci, f"{type(exc).__name__}: {str(exc)[:200]}", actions
    return ci, "ok", actions


def cmd_run_factors(a) -> None:
    uni, _prot, _space, bm, ctx, _cells, conditions = _factor_context()
    FACTOR_REC_DIR.mkdir(parents=True, exist_ok=True)
    pending = cd.pending_conditions(FACTOR_REGISTRY_DB, FACTOR_REC_DIR, conditions, ctx)
    if a.max_configs is not None:
        pending = pending[:a.max_configs]
    print(f"conditions total={len(conditions)} pending={len(pending)}", flush=True)
    t0, failed, totals = time.time(), [], {}
    with mp.get_context("spawn").Pool(a.workers, initializer=_init_worker, initargs=(uni, bm, ctx)) as pool:
        for n, (ci, status, actions) in enumerate(pool.imap_unordered(_eval_condition, pending), 1):
            for k, v in actions.items():
                totals[k] = totals.get(k, 0) + v
            if status != "ok":
                failed.append((ci, status))
            if n % 10 == 0 or status != "ok":
                print(f"{time.time() - t0:7.0f}s {n}/{len(pending)} condition {ci} {status}", flush=True)
    print(json.dumps({"conditions_ok": len(pending) - len(failed), "failed": len(failed), "factor_actions": totals},
                     sort_keys=True))
    if failed:
        raise SystemExit(f"fail-closed: {len(failed)} conditions hit faults; re-run run-factors to resume: {failed[:3]}")


def cmd_registry(_a) -> None:
    uni, prot, space, bm, ctx, cells, configs = _factor_context()
    records = cd.load_factor_records(FACTOR_REGISTRY_DB, FACTOR_REC_DIR, configs, ctx)
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
    sub.add_parser("freeze-factors").set_defaults(fn=cmd_freeze_factors)
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
