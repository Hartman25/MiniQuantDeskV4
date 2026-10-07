"""Confirmation cohort derivation from committed Pass-2 evidence, result-independent evaluation identity, the predeclaration
document and the runtime freeze guard. require_freeze() is the single gate every Confirmation data read must pass."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

import c1_protocol as cp

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
CENSUS = HERE.parents[0] / "alpha_edge_census_01"
PASS2_RESULTS = HERE.parents[0] / "alpha_edge_pass2_01" / "results"
UNIVERSE_JSON = CENSUS / "ALPHA_CENSUS_UNIVERSE_V2.json"
for _p in (str(CENSUS), str(HERE.parents[1] / "src")):
    if _p not in sys.path:
        sys.path.insert(0, _p)

PREDECL, COHORT, FREEZE_PROOF = (HERE / n for n in ("CONFIRMATION_PREDECLARATION.json", "CONFIRMATION_COHORT_MANIFEST.json",
                                                    "CONFIRMATION_FREEZE_PROOF.json"))
FREEZE_PREFIX = f"{cp.EXPERIMENT_ID}:CONFIRMATION_FREEZE:"
MISSION_START_HEAD = "9d3635cd45d963d0da6782d71c807f9b798143c3"
PASS2_FILES = ("PASS2_PROCESS_DISPOSITION.json", "conditional_robustness_ledger.jsonl", "pass2_summary.json",
               "survivor_rankings_readonly.json", "strategy_robustness_ledger.jsonl")
_EXP = "research-py/experiments/alpha_edge_confirmation_01"
_CENSUS = "research-py/experiments/alpha_edge_census_01"
_SRC = "research-py/src/mqk_research"
EXTERNAL_BOUND_SOURCES = (
    f"{_CENSUS}/signals.py", f"{_CENSUS}/conditional.py", f"{_CENSUS}/calendar_authority.py", f"{_CENSUS}/partitions.py",
    f"{_CENSUS}/search_space.py", f"{_CENSUS}/data.py", f"{_CENSUS}/ALPHA_CENSUS_UNIVERSE_V2.json",
    f"{_SRC}/factors/diagnostics.py", f"{_SRC}/factors/fdr.py", f"{_SRC}/factors/null_controls.py",
    f"{_SRC}/indicators/core.py", f"{_SRC}/data/alpaca_historical.py", f"{_SRC}/data/bars_provenance.py",
    "core-rs/crates/mqk-integrity/src/sessions.rs",
    *(f"research-py/experiments/alpha_edge_pass2_01/results/{n}" for n in PASS2_FILES))


class FreezeRefusal(RuntimeError):
    pass


CohortRefusal = FreezeRefusal


def sha_lf(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def load_json(path: Path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def iter_jsonl(path: Path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                yield json.loads(line)


def sorted_root(ids) -> str:
    h = hashlib.sha256()
    for t in sorted(ids):
        h.update(t.encode("ascii") + b"\n")
    return h.hexdigest()


def bound_sources(exp_dir: Path = HERE, repo: Path = REPO) -> list[str]:
    own = sorted(str(p.resolve().relative_to(Path(repo).resolve())).replace("\\", "/")
                 for p in Path(exp_dir).glob("c1_*.py"))
    return own + list(EXTERNAL_BOUND_SOURCES)


def source_sha256_lf(repo: Path = REPO, sources=None) -> dict:
    repo = Path(repo)
    return {s: sha_lf(repo / s) for s in (sources if sources is not None else bound_sources())}


# ------------------------------------------------------------------------------------------------------- cohort / identity

def derive_cohort(pass2_dir: Path = PASS2_RESULTS) -> list[dict]:
    """Exactly the committed Pass-2 Conditional survivors; any deviation REFUSES before a Confirmation read."""
    pass2_dir = Path(pass2_dir)
    disp = load_json(pass2_dir / "PASS2_PROCESS_DISPOSITION.json")
    for name, want in disp["bound_artifacts_sha256_lf"].items():
        if sha_lf(pass2_dir / name) != want:
            raise FreezeRefusal(f"Pass-2 artifact {name} differs from the hash bound by the process disposition")
    cond = disp["conditional"]
    if (cond["denominator"], cond["survivors"], cond["rejected"], cond["blocked"]) != (
            cp.PASS2_DENOMINATOR, cp.EXPECTED_COHORT, cp.PASS2_REJECTED, 0):
        raise FreezeRefusal("Pass-2 conditional counts differ from the frozen 135/18/117/0")
    rows = list(iter_jsonl(pass2_dir / "conditional_robustness_ledger.jsonl"))
    if len(rows) != cp.PASS2_DENOMINATOR:
        raise FreezeRefusal(f"Pass-2 conditional ledger has {len(rows)} rows, expected {cp.PASS2_DENOMINATOR}")
    surv = [r for r in rows if r["verdict"] == cp.PASS2_SURVIVOR_VERDICT]
    if len(surv) != cp.EXPECTED_COHORT:
        raise FreezeRefusal(f"cohort is {len(surv)}, expected {cp.EXPECTED_COHORT}: REFUSE before any Confirmation read")
    if len({r["factor_id"] for r in surv}) != len(surv):
        raise FreezeRefusal("duplicate factor_id in the Pass-2 survivor set")
    ranked = {r["factor_id"] for r in load_json(pass2_dir / "survivor_rankings_readonly.json")["conditional"]}
    if ranked != {r["factor_id"] for r in surv}:
        raise FreezeRefusal("Pass-2 survivor rankings disagree with the Pass-2 ledger survivor set")
    fams: dict = {}
    for r in surv:
        fams[r["family"]] = fams.get(r["family"], 0) + 1
    if dict(sorted(fams.items())) != cp.EXPECTED_FAMILY_COUNTS:
        raise FreezeRefusal(f"survivor family counts {fams} differ from the frozen {cp.EXPECTED_FAMILY_COUNTS}")
    out = []
    for r in surv:
        c1 = r["scenarios"]["C1"]
        if c1["status"] != "PASS" or c1["fdr_status"] != "complete" or c1["factor_id"] != r["factor_id"]:
            raise FreezeRefusal(f"{r['factor_id']}: Pass-2 C1 evidence is not a complete PASS")
        if r["horizon"] not in cp.HORIZONS:
            raise FreezeRefusal(f"{r['factor_id']}: horizon outside the frozen set")
        out.append({"factor_id": r["factor_id"], "condition_id": r["condition_id"], "edge_id": r["edge_id"],
                    "pass2_candidate_id": r["candidate_id"], "family": r["family"], "params": r["params"],
                    "horizon": r["horizon"], "direction": cp.DIRECTION,
                    "discovery_diagnostics": {"effect": c1["direction_adjusted_effect"], "p_value": c1["p_value"],
                                              "q_value": c1["q_value"], "event_count": c1["event_count"],
                                              "horizon_class": r["horizon_class"], "regime_class": r["regime_class"]}})
    return sorted(out, key=lambda e: e["factor_id"])


def data_provenance_identity() -> str:
    import calendar_authority as cal
    return cp.sha256_canonical({"request_contract": cp.REQUEST_CONTRACT, "universe_sha256_lf": sha_lf(UNIVERSE_JSON),
                                "calendar_contract": cal.CONTRACT_ID})


def fdr_family_identity(factor_ids) -> str:
    return cp.sha256_canonical({"factor_ids": sorted(factor_ids), "alpha": cp.FDR_ALPHA, "protocol": cp.FDR_PROTOCOL})


def evaluation_id(factor_id: str, family_identity: str, provenance_identity: str, protocol_id: str | None = None) -> str:
    """Result-independent: protocol + factor + partitions + data provenance + label/null protocols + FDR family."""
    return cp.sha256_canonical({
        "confirmation_protocol_id": protocol_id or cp.CONFIRMATION_PROTOCOL_ID, "factor_id": factor_id,
        "partitions": cp.PARTITION_IDENTITY, "data_provenance_identity": provenance_identity,
        "label_protocol": cp.LABEL_PROTOCOL, "null_protocol": cp.NULL_PROTOCOL, "fdr_family_identity": family_identity})[:32]


def trial_id_of(eval_id: str) -> str:
    return "c1-" + eval_id


def cohort_manifest(entries: list[dict], provenance_identity: str | None = None) -> dict:
    prov = provenance_identity if provenance_identity is not None else data_provenance_identity()
    fam = fdr_family_identity(e["factor_id"] for e in entries)
    cands = [{**e, "evaluation_id": evaluation_id(e["factor_id"], fam, prov)} for e in sorted(entries, key=lambda e: e["factor_id"])]
    return {"schema_version": "alpha_edge_confirmation_cohort_manifest_v1", "confirmation_protocol_id": cp.CONFIRMATION_PROTOCOL_ID,
            "candidates": cands, "candidate_count": len(cands), "cohort_root": sorted_root(e["factor_id"] for e in cands),
            "data_provenance_identity": prov, "fdr_family_identity": fam, "confirmation_attempt_count": 0}


def predeclaration(manifest: dict, *, starting_head: str, freeze_base_head: str) -> dict:
    return {
        "schema_version": "alpha_edge_confirmation_predeclaration_v1",
        "CONFIRMATION_PROTOCOL_ID": cp.CONFIRMATION_PROTOCOL_ID, "protocol": cp.build_protocol(),
        "starting_head": starting_head, "freeze_base_head": freeze_base_head,
        "cohort": {"count": manifest["candidate_count"], "root": manifest["cohort_root"],
                   "cohort_manifest_sha256": cp.sha256_canonical(manifest),
                   "fdr_family_identity": manifest["fdr_family_identity"]},
        "data_provenance_identity": manifest["data_provenance_identity"],
        "disclosures": [
            "2024 is CONTAMINATED_BY_REJECTED_RUN and is used only as indicator warm-up context: it contributes no "
            "observations, events, baseline rows, effects, p-values or FDR inputs.",
            "A signal whose lookback is longer than the available warm-up is undefined for its first bars; those rows are "
            "naturally excluded from the scored frame (no value is invented).",
            "The universe is the frozen 88-symbol current-registry snapshot: NOT point-in-time; survivorship caveat is mandatory.",
            "Corporate-action discovery uses process-date bounds up to the wall clock, so corporate-action metadata may name "
            "events after 2026-03-01; no bar value at or after 2026-03-01 is requested, retained or scored.",
            "Discovery effect/p/q are diagnostics only and never gate a Confirmation disposition.",
            "No Confirmation value was read before this freeze was committed."],
        "labels": cp.LABELS, "confirmation": "NOT_RUN", "confirmation_attempts_at_freeze": 0,
        "final_holdout": "RESERVED_UNCONSUMED", "promotion": "NOT_CLAIMED", "paper": "INACTIVE", "live": "DISABLED"}


def freeze_marker(manifest: dict) -> dict:
    return {"confirmation_protocol_id": cp.CONFIRMATION_PROTOCOL_ID, "cohort_root": manifest["cohort_root"],
            "candidate_count": manifest["candidate_count"], "fdr_family_identity": manifest["fdr_family_identity"],
            "data_provenance_identity": manifest["data_provenance_identity"]}


# ----------------------------------------------------------------------------------------------------------- freeze guard

def _git_rc(repo: Path, *args: str) -> int:
    return subprocess.run(["git", *args], cwd=repo, capture_output=True).returncode


def require_committed(paths, repo: Path) -> str:
    """Fail closed unless every path is tracked and identical to HEAD. Returns HEAD."""
    repo = Path(repo)
    for p in paths:
        rel = str(Path(p).resolve().relative_to(repo.resolve())).replace("\\", "/")
        if _git_rc(repo, "ls-files", "--error-unmatch", rel) != 0:
            raise FreezeRefusal(f"{rel} is not committed: Confirmation data access is refused before the freeze commit")
        if _git_rc(repo, "diff", "--quiet", "HEAD", "--", rel) != 0:
            raise FreezeRefusal(f"{rel} differs from HEAD: Confirmation data access is refused")
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()


def require_freeze(repo: Path = REPO, exp_dir: Path = HERE, *, derive=None, sources=None) -> dict:
    """Runtime freeze guard. Refuses unless the three freeze files exist and are committed at HEAD, the proof hashes match,
    the manifest equals a fresh derivation of exactly 18 candidates, the protocol equals the predeclared one, and every
    behavior-bearing source is committed, clean and equal to the hash bound by the proof."""
    repo, exp_dir = Path(repo), Path(exp_dir)
    files = [exp_dir / n for n in ("CONFIRMATION_PREDECLARATION.json", "CONFIRMATION_COHORT_MANIFEST.json",
                                   "CONFIRMATION_FREEZE_PROOF.json")]
    for f in files:
        if not f.exists():
            raise FreezeRefusal(f"{f.name} does not exist: Confirmation data access is refused before the freeze")
    head = require_committed(files, repo)
    predecl, manifest, proof = (load_json(f) for f in files)
    if proof["predeclaration_sha256_lf"] != sha_lf(files[0]) or proof["cohort_manifest_sha256_lf"] != sha_lf(files[1]):
        raise FreezeRefusal("freeze proof hashes do not match the predeclaration/manifest")
    if manifest["candidate_count"] != cp.EXPECTED_COHORT or len(manifest["candidates"]) != cp.EXPECTED_COHORT:
        raise FreezeRefusal("committed manifest is not exactly 18 candidates")
    if predecl["CONFIRMATION_PROTOCOL_ID"] != cp.CONFIRMATION_PROTOCOL_ID or predecl["protocol"] != cp.build_protocol() or \
            manifest["confirmation_protocol_id"] != cp.CONFIRMATION_PROTOCOL_ID:
        raise FreezeRefusal("protocol differs from the committed predeclaration (a value changed after the freeze)")
    if predecl["cohort"]["cohort_manifest_sha256"] != cp.sha256_canonical(manifest):
        raise FreezeRefusal("predeclaration cohort hash differs from the committed manifest")
    entries = (derive or derive_cohort)()
    if cohort_manifest(entries, manifest["data_provenance_identity"]) != manifest or \
            data_provenance_identity() != manifest["data_provenance_identity"]:
        raise FreezeRefusal("cohort re-derived from Pass-2 evidence differs from the committed manifest")
    srcs = list(sources) if sources is not None else bound_sources(exp_dir, repo)
    require_committed([repo / s for s in srcs], repo)
    if proof["source_sha256_lf"] != source_sha256_lf(repo, srcs):
        raise FreezeRefusal("a behavior-bearing source differs from the hash bound by the freeze proof")
    return {"head": head, "predeclaration": predecl, "manifest": manifest, "proof": proof}
