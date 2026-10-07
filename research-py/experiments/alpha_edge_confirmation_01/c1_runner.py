"""Confirmation controller: freeze, guarded acquisition, durable resumable evaluation, complete ledgers, read-only rankings.

Attempts live in a Confirmation-only ResearchResultStore (separate sqlite, experiment_id alpha_edge_confirmation_01): one
durable attempt per frozen factor, opened BEFORE computation, terminal evidence stored atomically with the finalize. The
Pass-1 and Pass-2 registries are never opened. A terminal evaluation is never re-executed; only `infrastructure_interrupted`
is retryable. Every Confirmation data read is preceded by c1_cohort.require_freeze()."""

from __future__ import annotations

import argparse
import contextlib
import json
import subprocess
import sys
import time
from pathlib import Path

import c1_cohort as cc
import c1_data as cd
import c1_eval as ce
import c1_protocol as cp
import conditional as cn  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

HERE, REPO = cc.HERE, cc.REPO
RUN = HERE.parents[1] / "runs" / cp.EXPERIMENT_ID
STORE_DB = RUN / "registry_confirmation.sqlite"
RESULTS = HERE / "results"
HYPOTHESIS = "c1-hypothesis-pass2-conditional-confirmation"


class ConfirmationRefusal(RuntimeError):
    pass


class ConfirmationDefect(RuntimeError):
    """An evaluation raised: durably failed; the run hard-stops (a defect is never retried by outcome)."""


def dump(path: Path, obj) -> None:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, sort_keys=True, indent=1, allow_nan=False) + "\n", encoding="utf-8", newline="\n")


def write_immutable(path: Path, doc: dict, what: str, *, ignore: tuple = ()) -> None:
    if path.exists():
        strip = lambda d: {k: v for k, v in d.items() if k not in ignore}  # noqa: E731
        if strip(cc.load_json(path)) != strip(json.loads(json.dumps(doc))):
            raise ConfirmationRefusal(f"{what} is immutable and differs from the regenerated authority")
        return
    dump(path, doc)


def write_ledger(name: str, recs: list[dict]) -> None:
    RESULTS.mkdir(exist_ok=True)
    with open(RESULTS / name, "w", encoding="utf-8", newline="\n") as f:
        for r in recs:
            f.write(json.dumps(r, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n")


# ------------------------------------------------------------------------------------------------------ store / freeze

def _marker_id(marker: dict) -> str:
    return cc.FREEZE_PREFIX + marker["cohort_root"][:32]


def register(store: ResearchResultStore, manifest: dict) -> dict:
    store.register_hypothesis(hypothesis_id=HYPOTHESIS, experiment_id=cp.EXPERIMENT_ID,
                              hypothesis_text="Independent Confirmation of the 18 Pass-2 Conditional survivors")
    store.register_trials_bulk([
        {"trial_id": cc.trial_id_of(c["evaluation_id"]), "experiment_id": cp.EXPERIMENT_ID, "hypothesis_id": HYPOTHESIS,
         "strategy_id": f"{cp.KIND}:{c['factor_id']}", "protocol_id": cp.CONFIRMATION_PROTOCOL_ID[:32],
         "identity": {"factor_id": c["factor_id"], "evaluation_id": c["evaluation_id"], "family": c["family"],
                      "params": c["params"], "horizon": c["horizon"], "direction": c["direction"],
                      "fdr_family_identity": manifest["fdr_family_identity"],
                      "data_provenance_identity": manifest["data_provenance_identity"]}}
        for c in manifest["candidates"]])
    marker = cc.freeze_marker(manifest)
    store.register_hypothesis(hypothesis_id=_marker_id(marker), experiment_id=cp.EXPERIMENT_ID,
                              hypothesis_text=json.dumps(marker, sort_keys=True, separators=(",", ":")))
    return marker


def require_frozen(store: ResearchResultStore, manifest: dict, *, allow_attempts: bool) -> dict:
    digest = store.trial_attempt_digest(cp.EXPERIMENT_ID)
    expected = {cc.trial_id_of(c["evaluation_id"]) for c in manifest["candidates"]}
    if set(digest) != expected:
        raise ConfirmationRefusal(f"registered != cohort: missing={len(expected - set(digest))} extra={len(set(digest) - expected)}")
    marker = cc.freeze_marker(manifest)
    with contextlib.closing(store._connect()) as con:  # noqa: SLF001 - read-only probe
        row = con.execute("select hypothesis_text from research_hypotheses where hypothesis_id=?", (_marker_id(marker),)).fetchone()
    if row is None or json.loads(row[0]) != marker:
        raise ConfirmationRefusal("Confirmation freeze marker absent or different; attempt before the freeze refused")
    attempts = sum(d["attempts"] for d in digest.values())
    if attempts and not allow_attempts:
        raise ConfirmationRefusal("attempts already exist; the freeze check demands attempts == 0")
    return {"registered": len(digest), "attempts": attempts, "succeeded": sum(d["succeeded"] for d in digest.values()),
            "started": sum(d["started"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values())}


def gate(freeze=None) -> tuple[dict, ResearchResultStore]:
    """Everything that must hold before ANY Confirmation read or attempt."""
    frz = cc.require_freeze(**(freeze or {}))
    store = ResearchResultStore(STORE_DB)
    require_frozen(store, frz["manifest"], allow_attempts=True)
    return frz, store


@contextlib.contextmanager
def exclusive_owner(name: str):
    """Process-lifetime OS lock: one controller per registry; released by the OS on exit or death."""
    RUN.mkdir(parents=True, exist_ok=True)
    with open(RUN / f"{name}.lock", "a+b") as fh:
        try:
            if sys.platform == "win32":
                import msvcrt
                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise ConfirmationRefusal("another Confirmation controller owns the registry; refusing to run") from None
        try:
            yield
        finally:
            if sys.platform == "win32":
                fh.seek(0)
                msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)


# ------------------------------------------------------------------------------------------------- durable execution

def assert_record_labels(rec: dict) -> None:
    for key, want in (("VALIDATION_STATUS", "NOT_VALIDATED"), ("PROMOTION_AUTHORITY", "NONE"), ("EXECUTABLE_PNL", False)):
        if rec.get(key) != want:
            raise ConfirmationRefusal(f"{rec.get('factor_id')}: record must carry {key}={want!r} (no validation, promotion or P&L claim)")
    ce.assert_labels(rec)


def terminal_record(store, cand: dict) -> dict | None:
    """The persisted terminal record iff a succeeded attempt exists; interrupted/started attempts are released as retryable
    infrastructure failures; any other failed attempt is never retried by outcome. A record carrying a foreign
    evaluation identity refuses before any mutation."""
    tid = cc.trial_id_of(cand["evaluation_id"])
    atts = store.list_attempts(tid)
    succ = [a for a in atts if a["status"] == "succeeded"]
    if succ:
        if len(succ) != 1 or any(a["attempt_index"] > succ[0]["attempt_index"] for a in atts):
            raise ConfirmationRefusal(f"{tid}: conflicting attempts after a terminal success; refusing to continue")
        rec = json.loads(succ[0]["result_summary_json"])
        if rec.get("evaluation_id") != cand["evaluation_id"] or rec.get("factor_id") != cand["factor_id"]:
            raise ConfirmationRefusal(f"{tid}: persisted record carries a foreign evaluation_id; refusing before mutation")
        return rec
    if any(a["status"] == "failed" and a["failure_reason"] != cp.INTERRUPTED_REASON for a in atts):
        raise ConfirmationRefusal(f"{tid}: failed with a reason other than {cp.INTERRUPTED_REASON!r}; never retried by outcome")
    stale = [{"attempt_id": a["attempt_id"], "status": "failed", "failure_reason": cp.INTERRUPTED_REASON}
             for a in atts if a["status"] == "started"]
    if stale:
        store.finalize_attempts_bulk(stale)
    return None


def execute(store, cands: list[dict], fn, *, batch: int = 6, log=print) -> dict:
    """Run every non-terminal candidate. Attempts are opened before evaluation and finalized one at a time; evaluation
    output never feeds back into any identity."""
    registered = set(store.trial_attempt_digest(cp.EXPERIMENT_ID))
    foreign = [c["factor_id"] for c in cands if cc.trial_id_of(c["evaluation_id"]) not in registered]
    if foreign:
        raise ConfirmationRefusal(f"evaluation identity not registered in the freeze: {foreign}; refused before mutation")
    pending = [c for c in cands if terminal_record(store, c) is None]
    done_before, ran = len(cands) - len(pending), 0
    for k in range(0, len(pending), batch):
        part = pending[k:k + batch]
        started = dict(zip((c["factor_id"] for c in part), store.begin_attempts_bulk(
            [cc.trial_id_of(c["evaluation_id"]) for c in part], origin=cp.ORIGIN, metadata={"batch": k // batch})))
        settled = set()
        for c in part:
            aid = started[c["factor_id"]][0]
            try:
                rec = fn(c)
                assert_record_labels(rec)
            except Exception as exc:  # noqa: BLE001 - durably failed, then the run hard-stops
                reason = f"{type(exc).__name__}: {str(exc)[:300]}"
                store.finalize_attempts_bulk([{"attempt_id": aid, "status": "failed", "failure_reason": reason}])
                settled.add(c["factor_id"])
                rest = [{"attempt_id": a, "status": "failed", "failure_reason": cp.INTERRUPTED_REASON}
                        for f, (a, _i) in started.items() if f not in settled]
                if rest:
                    store.finalize_attempts_bulk(rest)
                raise ConfirmationDefect(f"{c['factor_id']}: {reason}") from exc
            store.finalize_attempts_bulk([{"attempt_id": aid, "status": "succeeded", "result_summary": rec}])
            settled.add(c["factor_id"])
            ran += 1
        log(f"{min(k + batch, len(pending))}/{len(pending)} pending evaluations terminal")
    return {"candidates": len(cands), "terminal_before": done_before, "executed": ran}


def collect(store, cands: list[dict]) -> list[dict]:
    out = []
    for c in cands:
        rec = terminal_record(store, c)
        if rec is None:
            raise ConfirmationRefusal(f"{c['factor_id']}: not terminal; complete the run before reporting")
        out.append(rec)
    return out


# ----------------------------------------------------------------------------------------------------- data / universe

def load_universe(freeze=None) -> tuple[ce.ConfUniverse, dict]:
    """Per-symbol typed classification (every symbol accounted for) and the Confirmation-grid universe. SPY must be eligible."""
    import pandas as pd
    symbols = cc.load_json(cc.UNIVERSE_JSON)["symbols"]
    bars, cls = {}, []
    for s in symbols:
        sym_dir = RUN / "bars" / s
        status_path = sym_dir / "status.json"
        if not status_path.exists():
            raise ConfirmationRefusal(f"{s}: not acquired; run `acquire` first")
        c = cd.classify_symbol(s, cc.load_json(status_path), sym_dir, freeze=freeze)
        if c["disposition"] == cd.ELIGIBLE:
            bars[s] = cd.load_symbol_bars(sym_dir, freeze=freeze)[0]
        cls.append(c)
    if "SPY" not in bars:
        raise ConfirmationRefusal("SPY is not eligible: regime attribution unavailable (BLOCKED)")
    allts = pd.concat([b["end_ts"] for b in bars.values()])
    score_start, fence = ce.SCORE_START_TS, ce.FENCE_TS
    warm, reserve = allts[allts < score_start], allts[allts >= score_start]
    info = {"symbols": cls, "universe_count": len(symbols), "eligible": len(bars),
            "excluded_by_disposition": {d: sum(1 for c in cls if c["disposition"] == d)
                                        for d in sorted({c["disposition"] for c in cls} - {cd.ELIGIBLE})},
            "warmup_min": warm.min().isoformat() if len(warm) else None, "warmup_max": warm.max().isoformat() if len(warm) else None,
            "reserve_min": reserve.min().isoformat() if len(reserve) else None,
            "reserve_max": reserve.max().isoformat() if len(reserve) else None,
            "rows_total": int(len(allts)), "rows_at_or_after_fence": int((allts >= fence).sum())}
    if info["rows_at_or_after_fence"]:
        raise ce.LabelFenceBreach("loaded bars reach the final-holdout fence")
    return ce.ConfUniverse(bars), info


def cmd_acquire(_a) -> None:
    cc.require_freeze()
    symbols = cc.load_json(cc.UNIVERSE_JSON)["symbols"]
    with exclusive_owner("acquire"):
        t0, counts = time.time(), {}
        for i, s in enumerate(symbols, 1):
            st = cd.acquire_symbol(s, RUN / "bars" / s)
            counts[st["disposition"]] = counts.get(st["disposition"], 0) + 1
            if i % 11 == 0 or i == len(symbols):
                print(f"{time.time() - t0:6.0f}s {i}/{len(symbols)} {json.dumps(counts, sort_keys=True)}", flush=True)


def cmd_evaluate(_a) -> None:
    with exclusive_owner("evaluate"):
        frz, store = gate()
        cands = frz["manifest"]["candidates"]
        state: dict = {}

        def fn(entry: dict) -> dict:
            if "U" not in state:
                state["U"], info = load_universe()
                write_immutable(RUN / "universe_load.json", info, "universe load record")
                state["cache"] = cn.PermutationCache()
            return {**ce.evaluate_factor(state["U"], entry, state["cache"]), "kind": cp.KIND, **cp.LABELS}

        t0 = time.time()
        res = execute(store, cands, fn, log=lambda m: print(f"{time.time() - t0:6.0f}s {m}", flush=True))
        print(json.dumps(res, sort_keys=True))


# ------------------------------------------------------------------------------------------------------------- freeze

def cmd_freeze(_a) -> None:
    cc.require_committed([REPO / s for s in cc.bound_sources()], REPO)
    entries = cc.derive_cohort()
    manifest = cc.cohort_manifest(entries)
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
    predecl = cc.predeclaration(manifest, starting_head=cc.MISSION_START_HEAD, freeze_base_head=head)
    write_immutable(cc.COHORT, manifest, "cohort manifest")
    write_immutable(cc.PREDECL, predecl, "predeclaration", ignore=("freeze_base_head",))
    RUN.mkdir(parents=True, exist_ok=True)
    store = ResearchResultStore(STORE_DB)
    marker = register(store, manifest)
    gated = require_frozen(store, manifest, allow_attempts=False)
    if gated["attempts"] != 0 or gated["registered"] != cp.EXPECTED_COHORT:
        raise ConfirmationRefusal("freeze requires 18 registered evaluations and zero attempts")
    proof = {"schema_version": "alpha_edge_confirmation_freeze_proof_v1", "CONFIRMATION_PROTOCOL_ID": cp.CONFIRMATION_PROTOCOL_ID,
             "freeze_marker": marker, "registered_evaluations": gated["registered"], "confirmation_attempts_at_freeze": gated["attempts"],
             "cohort_root": manifest["cohort_root"], "predeclaration_sha256_lf": cc.sha_lf(cc.PREDECL),
             "cohort_manifest_sha256_lf": cc.sha_lf(cc.COHORT), "source_sha256_lf": cc.source_sha256_lf(),
             "confirmation_data_read_before_freeze": False}
    write_immutable(cc.FREEZE_PROOF, proof, "freeze proof")
    print(json.dumps({k: proof[k] for k in ("registered_evaluations", "confirmation_attempts_at_freeze", "cohort_root")}, sort_keys=True))


# ------------------------------------------------------------------------------------------------------------- report

def consumption_proof(records: list[dict], info: dict, statuses: list[dict]) -> dict:
    fence = ce.FENCE_TS
    raw_max = max((s["raw_max_t"] for s in statuses if s.get("raw_max_t")), default=None)
    if raw_max is not None and ce.pd.Timestamp(raw_max) >= fence:
        raise ce.LabelFenceBreach("provider returned a row at or after the fence")
    bounds = [r["scored_bounds"] for r in records if r["scored_bounds"]["rows"]]
    return {"schema_version": "alpha_edge_confirmation_consumption_v1", "CONFIRMATION_RESERVE_CONSUMED": True,
            "first_authorized_read_utc": min(s["first_request_utc"] for s in statuses),
            "requested_bounds": {"start_utc": cp.REQUEST_CONTRACT["start_utc"], "end_utc_exclusive": cp.REQUEST_CONTRACT["end_utc_exclusive"]},
            "provider_bar_requests": sum(s.get("bar_requests", 0) for s in statuses),
            "provider_raw_rows": sum(s.get("raw_rows", 0) for s in statuses), "provider_raw_max_t": raw_max,
            "warmup_min": info["warmup_min"], "warmup_max": info["warmup_max"],
            "actual_confirmation_min": info["reserve_min"], "actual_confirmation_max": info["reserve_max"],
            "max_scored_observation": max(b["max_period"] for b in bounds), "min_scored_observation": min(b["min_period"] for b in bounds),
            "max_label_endpoint": max(b["max_label_end"] for b in bounds),
            "scored_rows_by_factor_total": sum(b["rows"] for b in bounds),
            "final_holdout_rows_read": 0, "final_holdout_rows_scored": 0, "rows_at_or_after_fence_loaded": info["rows_at_or_after_fence"],
            "eligible_symbols": info["eligible"], "universe_symbols": info["universe_count"],
            "excluded_by_disposition": info["excluded_by_disposition"], **cp.LABELS}


def cmd_report(_a) -> None:
    frz, store = gate()
    manifest = frz["manifest"]
    cands = manifest["candidates"]
    recs = collect(store, cands)
    if len(recs) != cp.EXPECTED_COHORT:
        raise ConfirmationRefusal("incomplete denominator")
    fin = ce.finalize(cands, {r["factor_id"]: r for r in recs})
    rows = fin["rows"]
    for r in rows:
        assert_record_labels(r)
    info = cc.load_json(RUN / "universe_load.json")
    symbols = cc.load_json(cc.UNIVERSE_JSON)["symbols"]
    statuses = [cc.load_json(RUN / "bars" / s / "status.json") for s in symbols]
    RESULTS.mkdir(exist_ok=True)
    dump(RESULTS / "confirmation_protocol.json", {"CONFIRMATION_PROTOCOL_ID": cp.CONFIRMATION_PROTOCOL_ID, "protocol": cp.build_protocol()})
    dump(RESULTS / "confirmation_cohort.json", manifest)
    dump(RESULTS / "confirmation_data_provenance.json", {
        "schema_version": "alpha_edge_confirmation_data_provenance_v1", "request_contract": cp.REQUEST_CONTRACT,
        "data_provenance_identity": manifest["data_provenance_identity"], "universe_sha256_lf": cc.sha_lf(cc.UNIVERSE_JSON),
        "calendar_contract": ce.cal.CONTRACT_ID, "symbols": info["symbols"], "eligible": info["eligible"],
        "universe_count": info["universe_count"], "excluded_by_disposition": info["excluded_by_disposition"],
        "acquisition": [{k: s.get(k) for k in ("symbol", "disposition", "rows", "first_end_ts", "last_end_ts", "bar_requests",
                                                "raw_rows", "raw_max_t", "infrastructure_attempts")} for s in statuses],
        "survivorship_caveat": "frozen 88-symbol current-registry snapshot; not point-in-time"})
    write_ledger("confirmation_factor_ledger.jsonl", rows)
    dump(RESULTS / "confirmation_fdr_report.json", {**fin["fdr"], "fdr_family_identity": manifest["fdr_family_identity"],
                                                    "cohort_root": manifest["cohort_root"]})
    dump(RESULTS / "confirmation_summary.json", ce.summarize(rows))
    dump(RESULTS / "confirmation_rankings_readonly.json", ce.rankings(rows))
    dump(RESULTS / "confirmation_consumption_proof.json", consumption_proof(recs, info, statuses))
    digest = store.trial_attempt_digest(cp.EXPERIMENT_ID)
    write_ledger("confirmation_attempt_ledger.jsonl", [
        {"factor_id": c["factor_id"], "evaluation_id": c["evaluation_id"], "trial_id": cc.trial_id_of(c["evaluation_id"]),
         "attempts": [{"attempt_index": a["attempt_index"], "status": a["status"], "failure_reason": a["failure_reason"],
                       "origin": a["origin"]} for a in store.list_attempts(cc.trial_id_of(c["evaluation_id"]))]}
        for c in cands])
    dump(RESULTS / "confirmation_attempt_ledger_proof.json", {
        "registered": len(digest), "attempts": sum(d["attempts"] for d in digest.values()),
        "succeeded": sum(d["succeeded"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values()),
        "started": sum(d["started"] for d in digest.values()), "max_attempts_per_evaluation": max(d["attempts"] for d in digest.values())})
    print(json.dumps(ce.summarize(rows)["by_status"], sort_keys=True))


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name, fn in (("freeze", cmd_freeze), ("acquire", cmd_acquire), ("evaluate", cmd_evaluate), ("report", cmd_report)):
        sub.add_parser(name).set_defaults(fn=fn)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
