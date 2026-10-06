"""Pass-2 durable controller: freeze, gated resumable execution, complete ledgers, summaries, read-only rankings.

Attempts live in a Pass-2-only ResearchResultStore (separate sqlite, experiment_id alpha_edge_pass2_01): one durable attempt
per robustness candidate, opened BEFORE evaluation, terminal result stored atomically with the finalize. Pass-1 registries
are only ever opened mode=ro. A terminal candidate is never re-executed; only `infrastructure_interrupted` is retryable."""

from __future__ import annotations

import argparse
import contextlib
import json
import multiprocessing as mp
import os
import subprocess
import sys
import time
from pathlib import Path

import p2_pass1 as p1
import p2_protocol as pp
import p2_cohort as pc
import p2_conditional as p2c
import p2_strategy as p2s
import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import search_space as ss  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
RUN2 = HERE.parents[1] / "runs" / "alpha_edge_pass2_01"
STORE_DB = RUN2 / "registry_pass2.sqlite"
PREDECL, COHORT, FREEZE_PROOF = (HERE / n for n in ("PASS2_PREDECLARATION.json", "PASS2_COHORT_MANIFEST.json",
                                                    "PASS2_FREEZE_PROOF.json"))
RESULTS = HERE / "results"
HYP = {pp.KIND_STRATEGY: "p2-hypothesis-strategy-robustness", pp.KIND_CONDITIONAL: "p2-hypothesis-conditional-robustness"}
_W: dict = {}


class Pass2Refusal(RuntimeError):
    pass


class Pass2Defect(RuntimeError):
    """A candidate evaluation raised: durably failed, run hard-stops (defects are never retried by outcome)."""


def dump(path: Path, obj) -> None:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def write_immutable(path: Path, doc: dict, what: str, *, ignore: tuple = ()) -> None:
    if path.exists():
        old = p1.load_json(path)
        strip = lambda d: {k: v for k, v in d.items() if k not in ignore}  # noqa: E731
        if strip(old) != strip(json.loads(json.dumps(doc))):
            raise Pass2Refusal(f"{what} is immutable and differs from the regenerated authority")
        return
    dump(path, doc)


# ----------------------------------------------------------------------------------------------------- world / cohorts

def load_world() -> dict:
    import run_census as rc  # accepted Pass-1 setup authority (re-derives and verifies every frozen manifest)
    uni, part, prot, space, grammar, bm, ctx, cells = rc._setup()  # noqa: SLF001
    conditions = ce.cell_conditions(cells)
    edges = list(p1.iter_jsonl(p1.RUN_DIR / "edge_registry_v3.jsonl"))
    return {"uni": uni, "prot": prot, "space": space, "bm": bm, "ctx": ctx, "cells": cells, "conditions": conditions,
            "edges": edges, "sledger": list(p1.iter_jsonl(p1.RUN_DIR / "search_ledger_v3.jsonl")),
            "fledger": list(p1.iter_jsonl(p1.RUN_DIR / "factor_ledger_v3.jsonl")),
            "ids": {k: space[k] for k in pc.PASS1_ID_KEYS}}


def derive_manifest(w: dict) -> dict:
    strat = pc.strategy_cohort(w["edges"], w["cells"], w["sledger"])
    cond = pc.conditional_cohort(w["edges"], w["fledger"], w["conditions"], set(cd.expected_factor_ids(w["conditions"], w["ctx"])))
    return pc.cohort_manifest(strat, cond)


def all_candidates(manifest: dict) -> list[dict]:
    return manifest["strategy"] + manifest["conditional"]


def register(store: ResearchResultStore, manifest: dict, ids: dict, binding: dict) -> dict:
    for kind, text in ((pp.KIND_STRATEGY, "Pass-2 robustness of Pass-1 DISCOVERED_MODERATE StrategyEdges"),
                       (pp.KIND_CONDITIONAL, "Pass-2 robustness of Pass-1 DISCOVERED_STRONG ConditionalEdges")):
        store.register_hypothesis(hypothesis_id=HYP[kind], experiment_id=pp.EXPERIMENT_ID, hypothesis_text=text)
    store.register_trials_bulk([
        {"trial_id": pc.trial_id_of(c["candidate_id"]), "experiment_id": pp.EXPERIMENT_ID, "hypothesis_id": HYP[c["kind"]],
         "strategy_id": f"{c['kind']}:{c.get('trial_id') or c['factor_id']}", "protocol_id": pp.PASS2_PROTOCOL_ID[:32],
         "identity": pc.candidate_identity(c, ids)} for c in all_candidates(manifest)])
    marker = pc.freeze_marker(manifest, binding)
    store.register_hypothesis(hypothesis_id=pc.FREEZE_PREFIX + marker["strategy_cohort_root"] + marker["conditional_cohort_root"][:16],
                              experiment_id=pp.EXPERIMENT_ID, hypothesis_text=json.dumps(marker, sort_keys=True, separators=(",", ":")))
    return marker


def require_frozen(store: ResearchResultStore, manifest: dict, binding: dict, *, allow_attempts: bool) -> dict:
    digest = store.trial_attempt_digest(pp.EXPERIMENT_ID)
    expected = {pc.trial_id_of(c["candidate_id"]) for c in all_candidates(manifest)}
    if set(digest) != expected:
        raise Pass2Refusal(f"registered != cohort: missing={len(expected - set(digest))} extra={len(set(digest) - expected)}")
    marker = pc.freeze_marker(manifest, binding)
    hid = pc.FREEZE_PREFIX + marker["strategy_cohort_root"] + marker["conditional_cohort_root"][:16]
    with contextlib.closing(store._connect()) as con:  # noqa: SLF001 - read-only probe
        row = con.execute("select hypothesis_text from research_hypotheses where hypothesis_id=?", (hid,)).fetchone()
    if row is None or json.loads(row[0]) != marker:
        raise Pass2Refusal("Pass-2 freeze marker absent or different; attempt before the freeze refused")
    attempts = sum(d["attempts"] for d in digest.values())
    if attempts and not allow_attempts:
        raise Pass2Refusal("attempts already exist; the freeze check demands attempts == 0")
    return {"registered": len(digest), "attempts": attempts, "succeeded": sum(d["succeeded"] for d in digest.values()),
            "started": sum(d["started"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values())}


def gate(w: dict) -> tuple[dict, dict, ResearchResultStore]:
    """Everything that must hold before ANY robustness attempt: the freeze is committed, the protocol and Pass-1 evidence
    equal the predeclaration, the cohorts recomputed from Pass-1 equal the committed manifest, the store is frozen."""
    head = pc.require_committed([PREDECL, COHORT, FREEZE_PROOF], REPO)
    predecl, committed = p1.load_json(PREDECL), p1.load_json(COHORT)
    if predecl["PASS2_PROTOCOL_ID"] != pp.PASS2_PROTOCOL_ID or predecl["protocol"] != pp.build_protocol():
        raise Pass2Refusal("protocol differs from the committed predeclaration (a threshold changed after the freeze)")
    p1.verify_binding(predecl["pass1_evidence"])
    manifest = derive_manifest(w)
    if manifest != committed or manifest["strategy_cohort_root"] != predecl["cohort"]["strategy_cohort_root"] or \
            manifest["conditional_cohort_root"] != predecl["cohort"]["conditional_cohort_root"] or \
            pp.sha256_canonical(committed) != predecl["cohort"]["cohort_manifest_sha256"]:
        raise Pass2Refusal("cohort differs from the committed manifest/predeclaration")
    store = ResearchResultStore(STORE_DB)
    require_frozen(store, manifest, predecl["pass1_evidence"], allow_attempts=True)
    return manifest, predecl, store


@contextlib.contextmanager
def exclusive_owner(name: str):
    """Process-lifetime OS lock: one controller per Pass-2 registry; released by the OS on exit or death."""
    RUN2.mkdir(parents=True, exist_ok=True)
    with open(RUN2 / f"{name}.lock", "a+b") as fh:
        try:
            if sys.platform == "win32":
                import msvcrt
                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise Pass2Refusal("another Pass-2 controller owns the registry; refusing to run") from None
        try:
            yield
        finally:
            if sys.platform == "win32":
                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)


# ------------------------------------------------------------------------------------------------- durable execution

def assert_labels(rec: dict) -> None:
    if rec.get("VALIDATION_STATUS") != "NOT_VALIDATED" or rec.get("PROMOTION_AUTHORITY") != "NONE" or \
            (rec["kind"] == pp.KIND_CONDITIONAL and rec.get("executable_pnl") is not False):
        raise Pass2Refusal(f"{rec.get('candidate_id')}: a Pass-2 record may not claim validation, promotion or executable P&L")


def terminal_record(store, cand: dict) -> dict | None:
    """The persisted terminal record iff a succeeded attempt exists; interrupted/started attempts are released as retryable
    infrastructure failures; any other failed attempt (a defect or poor result) is never retried."""
    tid = pc.trial_id_of(cand["candidate_id"])
    atts = store.list_attempts(tid)
    succ = [a for a in atts if a["status"] == "succeeded"]
    if succ:
        if len(succ) != 1 or any(a["attempt_index"] > succ[0]["attempt_index"] for a in atts):
            raise Pass2Refusal(f"{tid}: conflicting attempts after a terminal success; refusing to continue")
        rec = json.loads(succ[0]["result_summary_json"])
        if rec.get("candidate_id") != cand["candidate_id"]:
            raise Pass2Refusal(f"{tid}: persisted record is not this candidate's")
        return rec
    stale = [{"attempt_id": a["attempt_id"], "status": "failed", "failure_reason": pp.INTERRUPTED_REASON}
             for a in atts if a["status"] == "started"]
    if stale:
        store.finalize_attempts_bulk(stale)
    if any(a["status"] == "failed" and a["failure_reason"] != pp.INTERRUPTED_REASON for a in atts):
        raise Pass2Refusal(f"{tid}: failed with a reason other than {pp.INTERRUPTED_REASON!r}; never retried by outcome")
    return None


def _safe(fn, task):
    try:
        return "ok", task["cand"]["candidate_id"], fn(task)
    except Exception as exc:  # noqa: BLE001 - reported to the parent which durably fails the attempt
        return "err", task["cand"]["candidate_id"], f"{type(exc).__name__}: {str(exc)[:300]}"


def _worker(task):
    return _safe(_W["fn"], task)


def _worker_init(init, args):
    _W["fn"] = init(*args)


def execute(store, cands: list[dict], tasks, fn, *, pool=None, batch: int = 24, log=print) -> dict:
    """Run every non-terminal candidate. `tasks`: candidate_id -> task dict. Attempts are opened before evaluation,
    finalized one at a time; evaluation results never feed back into any identity."""
    pending = [c for c in cands if terminal_record(store, c) is None]
    done_before = len(cands) - len(pending)
    ran = 0
    for k in range(0, len(pending), batch):
        part = pending[k:k + batch]
        started = dict(zip((c["candidate_id"] for c in part), store.begin_attempts_bulk(
            [pc.trial_id_of(c["candidate_id"]) for c in part], origin=pp.ORIGIN, metadata={"batch": k // batch})))
        work = [tasks[c["candidate_id"]] for c in part]
        outcomes = (pool.imap_unordered(_worker, work) if pool else (_safe(fn, t) for t in work))
        settled = set()
        for status, cid, payload in outcomes:
            aid = started[cid][0]
            if status == "ok":
                assert_labels(payload)
                store.finalize_attempts_bulk([{"attempt_id": aid, "status": "succeeded", "result_summary": payload}])
                settled.add(cid)
                ran += 1
                continue
            store.finalize_attempts_bulk([{"attempt_id": aid, "status": "failed", "failure_reason": payload}])
            settled.add(cid)
            rest = [{"attempt_id": a, "status": "failed", "failure_reason": pp.INTERRUPTED_REASON}
                    for c, (a, _i) in started.items() if c not in settled]
            if rest:
                store.finalize_attempts_bulk(rest)
            raise Pass2Defect(f"{cid}: {payload}")
        log(f"{min(k + batch, len(pending))}/{len(pending)} pending candidates terminal")
    return {"candidates": len(cands), "terminal_before": done_before, "executed": ran}


def _fence_log(proof: dict, name: str) -> None:
    RUN2.mkdir(parents=True, exist_ok=True)
    with open(RUN2 / "fence_proofs.jsonl", "a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"run": name, **proof}, sort_keys=True) + "\n")


def cmd_run_strategy(_a) -> None:
    w = load_world()
    with exclusive_owner("run_strategy"):
        manifest, _predecl, store = gate(w)
        U, fence = p1.load_discovery_universe(w["uni"], w["bm"])
        _fence_log(fence, "strategy")
        sc = p2s.build_context(U, ce.symbol_meta(w["bm"], w["prot"]), w["cells"], w["sledger"])
        edge = {e["trial_id"]: e for e in w["edges"] if e["kind"] == "STRATEGY_EDGE"}
        tasks = {c["candidate_id"]: {"cand": c, "metrics": edge[c["trial_id"]]["metrics"]} for c in manifest["strategy"]}
        t0 = time.time()
        res = execute(store, manifest["strategy"], tasks, lambda t: p2s.evaluate_strategy(sc, t["cand"], t["metrics"]),
                      log=lambda m: print(f"{time.time() - t0:6.0f}s {m}", flush=True))
        print(json.dumps(res, sort_keys=True))


def _cond_init(uni, bm, ctx, conditions, fledger, fdr):
    U, fence = p1.load_discovery_universe(uni, bm)
    cc = p2c.build_context(U, ctx, conditions, fledger, fdr)
    return lambda t: p2c.evaluate_conditional(cc, t["cand"], t["att"], t["rec1"], t["edge"])


def cmd_run_conditional(a) -> None:
    w = load_world()
    with exclusive_owner("run_conditional"):
        manifest, _predecl, store = gate(w)
        fdr = p1.load_json(p1.RUN_DIR / "factor_fdr_report_v3.json")
        fstore = p1.ReadOnlyStore(p1.FACTOR_REGISTRY)
        edge = {e["factor_id"]: e for e in w["edges"] if e["kind"] == "CONDITIONAL_EDGE"}
        tasks = {}
        for c in manifest["conditional"]:
            atts = fstore.list_factor_evaluation_attempts(c["factor_id"])
            rec1 = cd.read_factor_record(p1.FACTOR_REC_DIR, c["factor_id"])
            if not atts or atts[-1]["status"] != "succeeded" or rec1 is None:
                raise Pass2Refusal(f"{c['factor_id']}: authoritative Pass-1 attempt/record unavailable")
            tasks[c["candidate_id"]] = {"cand": c, "att": atts[-1], "rec1": rec1, "edge": edge[c["factor_id"]]}
        _U, fence = p1.load_discovery_universe(w["uni"], w["bm"])
        _fence_log(fence, "conditional")
        init_args = (w["uni"], w["bm"], w["ctx"], w["conditions"], w["fledger"], fdr)
        t0 = time.time()
        log = lambda m: print(f"{time.time() - t0:6.0f}s {m}", flush=True)  # noqa: E731
        if a.workers > 1:
            with mp.get_context("spawn").Pool(a.workers, initializer=_worker_init, initargs=(_cond_init, init_args)) as pool:
                res = execute(store, manifest["conditional"], tasks, None, pool=pool, batch=a.workers * 2, log=log)
        else:
            fn = _cond_init(*init_args)
            res = execute(store, manifest["conditional"], tasks, fn, batch=8, log=log)
        print(json.dumps(res, sort_keys=True))


# ------------------------------------------------------------------------------------------------------------- freeze

def cmd_freeze(_a) -> None:
    w = load_world()
    binding = p1.pass1_binding()
    manifest = derive_manifest(w)
    U, fence = p1.load_discovery_universe(w["uni"], w["bm"])
    del U
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
    predecl = pc.predeclaration(manifest, binding, starting_head=pc.MISSION_START_HEAD, freeze_base_head=head,
                                pass1_ids=w["ids"], universe=w["uni"], ctx=w["ctx"], fence=fence)
    write_immutable(COHORT, manifest, "cohort manifest")
    write_immutable(PREDECL, predecl, "predeclaration", ignore=("freeze_base_head",))
    store = ResearchResultStore(STORE_DB)
    marker = register(store, manifest, w["ids"], binding)
    gate_ = require_frozen(store, manifest, binding, allow_attempts=False)
    proof = {"schema_version": "alpha_edge_pass2_freeze_proof_v1", "PASS2_PROTOCOL_ID": pp.PASS2_PROTOCOL_ID,
             "freeze_marker": marker, "registered_candidates": gate_["registered"], "robustness_attempts_at_freeze": gate_["attempts"],
             "strategy_cohort": manifest["strategy_count"], "conditional_cohort": manifest["conditional_count"],
             "strategy_cohort_root": manifest["strategy_cohort_root"], "conditional_cohort_root": manifest["conditional_cohort_root"],
             "predeclaration_sha256_lf": p1.sha_lf(PREDECL), "cohort_manifest_sha256_lf": p1.sha_lf(COHORT),
             "pass1_attempt_counts": binding["attempt_counts"], "input_fence": fence}
    if gate_["attempts"] != 0 or gate_["registered"] != pp.EXPECTED_STRATEGY_COHORT + pp.EXPECTED_CONDITIONAL_COHORT:
        raise Pass2Refusal("freeze requires 924 registered candidates and zero robustness attempts")
    write_immutable(FREEZE_PROOF, proof, "freeze proof")
    print(json.dumps({k: proof[k] for k in ("registered_candidates", "robustness_attempts_at_freeze", "strategy_cohort",
                                            "conditional_cohort")}, sort_keys=True))


# ------------------------------------------------------------------------------------------------ report / summaries

def collect(store, cands: list[dict]) -> list[dict]:
    out = []
    for c in cands:
        rec = terminal_record(store, c)
        if rec is None:
            raise Pass2Refusal(f"{c['candidate_id']}: not terminal; complete the run before reporting")
        out.append(rec)
    return out


def _tally(items, key):
    out: dict = {}
    for it in items:
        k = key(it)
        out[k] = out.get(k, 0) + 1
    return dict(sorted(out.items(), key=lambda kv: str(kv[0])))


def summarize(strat: list[dict], cond: list[dict]) -> dict:
    gates_s, gates_c = pp.STRATEGY_HARD_GATES, pp.CONDITIONAL_HARD_GATES
    sv, cv = pp.VERDICTS_STRATEGY, pp.VERDICTS_CONDITIONAL
    sur_s = [r for r in strat if r["verdict"] == sv[0]]
    sur_c = [r for r in cond if r["verdict"] == cv[0]]

    def fails(records, gates):
        return {g: sum(g in r["failed_gates"] for r in records) for g in gates}

    return {
        "schema_version": "alpha_edge_pass2_summary_v1", "PASS2_PROTOCOL_ID": pp.PASS2_PROTOCOL_ID, **pp.LABELS,
        "strategy": {"denominator": len(strat), "verdicts": _tally(strat, lambda r: r["verdict"]),
                     "failures_by_scenario": fails(strat, gates_s),
                     "not_applicable_by_scenario": _tally([g for r in strat for g in r["not_applicable"]], lambda g: g),
                     "blocked_by_scenario": _tally([g for r in strat for g in r["blocked_gates"]], lambda g: g),
                     "failure_profiles": _tally(strat, lambda r: ",".join(r["failed_gates"]) or "NONE"),
                     "failures_by_family": {f: fails([r for r in strat if r["family"] == f], gates_s)
                                            for f in sorted({r["family"] for r in strat})},
                     "verdicts_by_family": {f: _tally([r for r in strat if r["family"] == f], lambda r: r["verdict"])
                                            for f in sorted({r["family"] for r in strat})},
                     "verdicts_by_replication_class": {c: _tally([r for r in strat if r["replication_class"] == c],
                                                                 lambda r: r["verdict"])
                                                       for c in sorted({r["replication_class"] for r in strat})},
                     "failures_by_replication_class": {c: fails([r for r in strat if r["replication_class"] == c], gates_s)
                                                       for c in sorted({r["replication_class"] for r in strat})},
                     "survivors_by_family": _tally(sur_s, lambda r: r["family"]),
                     "survivors_by_replication_class": _tally(sur_s, lambda r: r["replication_class"]),
                     "survivors_by_3x_cost_diagnostic": _tally(sur_s, lambda r: r["pass_3x_cost_diagnostic"]),
                     "diagnostic_3x_pass_all_candidates": _tally(strat, lambda r: r["pass_3x_cost_diagnostic"]),
                     "survivors_by_symbol": _tally(sur_s, lambda r: r["symbol"])},
        "conditional": {"denominator": len(cond), "verdicts": _tally(cond, lambda r: r["verdict"]),
                        "failures_by_scenario": fails(cond, gates_c),
                        "not_applicable_by_scenario": _tally([g for r in cond for g in r["not_applicable"]], lambda g: g),
                        "blocked_by_scenario": _tally([g for r in cond for g in r["blocked_gates"]], lambda g: g),
                        "failure_profiles": _tally(cond, lambda r: ",".join(r["failed_gates"]) or "NONE"),
                        "failures_by_family": {f: fails([r for r in cond if r["family"] == f], gates_c)
                                               for f in sorted({r["family"] for r in cond})},
                        "failures_by_horizon": {str(h): fails([r for r in cond if r["horizon"] == h], gates_c)
                                                for h in pp.HORIZONS},
                        "verdicts_by_horizon": {str(h): _tally([r for r in cond if r["horizon"] == h], lambda r: r["verdict"])
                                                for h in pp.HORIZONS},
                        "survivors_by_family": _tally(sur_c, lambda r: r["family"]),
                        "survivors_by_horizon": _tally(sur_c, lambda r: r["horizon"]),
                        "survivors_by_regime_class": _tally(sur_c, lambda r: r["regime_class"]),
                        "survivors_by_horizon_support_class": _tally(sur_c, lambda r: r["horizon_class"]),
                        "regime_class_all_candidates": _tally(cond, lambda r: r["regime_class"]),
                        "horizon_class_all_candidates": _tally(cond, lambda r: r["horizon_class"])}}


def write_ledger(name: str, recs: list[dict]) -> None:
    RESULTS.mkdir(exist_ok=True)
    with open(RESULTS / name, "w", encoding="utf-8", newline="\n") as f:
        for r in recs:
            f.write(json.dumps(r, sort_keys=True, separators=(",", ":")) + "\n")


def cmd_ledger(a) -> None:
    """Write the complete (survivors AND rejected) ledger of one kind straight from the durable attempts."""
    w = load_world()
    manifest, _predecl, store = gate(w)
    kind = a.kind
    recs = collect(store, manifest[kind])
    want = pp.EXPECTED_STRATEGY_COHORT if kind == "strategy" else pp.EXPECTED_CONDITIONAL_COHORT
    if len(recs) != want:
        raise Pass2Refusal("incomplete denominator")
    write_ledger(f"{kind}_robustness_ledger.jsonl", recs)
    print(json.dumps({kind: _tally(recs, lambda r: r["verdict"])}, sort_keys=True))


def cmd_report(_a) -> None:
    w = load_world()
    manifest, predecl, store = gate(w)
    strat, cond = collect(store, manifest["strategy"]), collect(store, manifest["conditional"])
    if len(strat) != pp.EXPECTED_STRATEGY_COHORT or len(cond) != pp.EXPECTED_CONDITIONAL_COHORT:
        raise Pass2Refusal("incomplete denominator")
    write_ledger("strategy_robustness_ledger.jsonl", strat)
    write_ledger("conditional_robustness_ledger.jsonl", cond)
    summary = summarize(strat, cond)
    fences = [json.loads(line) for line in open(RUN2 / "fence_proofs.jsonl", encoding="utf-8")]
    summary["input_fence"] = {"confirmation_rows_read": sum(f["rows_in_forbidden_partitions"] for f in fences),
                              "final_holdout_rows_read": sum(f["rows_in_forbidden_partitions"] for f in fences),
                              "max_end_ts_read": max(f["max_end_ts"] for f in fences), "loads": len(fences)}
    dump(RESULTS / "pass2_summary.json", summary)
    ranked_s, ranked_c = p2s.rank_strategy_survivors(strat), p2c.rank_conditional_survivors(cond)
    dump(RESULTS / "survivor_rankings_readonly.json", {
        "ranking_is_not_selection_authority": True, "consumes_confirmation_data": False,
        "strategy": [{"rank": i + 1, "trial_id": r["trial_id"], "family": r["family"], "symbol": r["symbol"],
                      "params": r["params"], "replication_class": r["replication_class"], **r["rank_fields"]}
                     for i, r in enumerate(ranked_s)],
        "conditional": [{"rank": i + 1, "factor_id": r["factor_id"], "family": r["family"], "params": r["params"],
                         "horizon": r["horizon"], "regime_class": r["regime_class"], "horizon_class": r["horizon_class"],
                         **r["rank_fields"]} for i, r in enumerate(ranked_c)]})
    after = p1.verify_binding(predecl["pass1_evidence"])
    dump(RESULTS / "pass1_immutability_proof.json", {
        "before_equals_after": after == predecl["pass1_evidence"], "predeclared_binding": predecl["pass1_evidence"],
        "recomputed_after": after, "new_pass1_strategy_trials": 0, "new_pass1_factors": 0,
        "strategy_attempts_before_after": [pp.EXPECTED_STRATEGY_ATTEMPTS, after["attempt_counts"]["strategy_total"]],
        "conditional_v3_attempts_before_after": [pp.EXPECTED_CONDITIONAL_ATTEMPTS, after["attempt_counts"]["conditional_v3_total"]]})
    digest = store.trial_attempt_digest(pp.EXPERIMENT_ID)
    dump(RESULTS / "attempt_ledger_proof.json", {
        "registered": len(digest), "attempts": sum(d["attempts"] for d in digest.values()),
        "succeeded": sum(d["succeeded"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values()),
        "started": sum(d["started"] for d in digest.values()), "max_attempts_per_candidate": max(d["attempts"] for d in digest.values())})
    print(json.dumps({"strategy": summary["strategy"]["verdicts"], "conditional": summary["conditional"]["verdicts"]}, sort_keys=True))


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("freeze").set_defaults(fn=cmd_freeze)
    sub.add_parser("run-strategy").set_defaults(fn=cmd_run_strategy)
    r = sub.add_parser("run-conditional")
    r.add_argument("--workers", type=int, default=4)
    r.set_defaults(fn=cmd_run_conditional)
    lg = sub.add_parser("ledger")
    lg.add_argument("--kind", choices=("strategy", "conditional"), required=True)
    lg.set_defaults(fn=cmd_ledger)
    sub.add_parser("report").set_defaults(fn=cmd_report)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
