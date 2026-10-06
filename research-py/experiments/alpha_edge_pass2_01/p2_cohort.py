"""Pass-2 advancement cohorts, result-independent candidate identity, the predeclaration document and the freeze gate.
The cohorts are fixed by Pass-1 class alone (789 DISCOVERED_MODERATE StrategyEdges + 135 DISCOVERED_STRONG
ConditionalEdges) and frozen before robustness attempt #1."""

from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path

import p2_pass1 as p1
import p2_protocol as pp
import edge_registry as er  # noqa: E402
import search_space as ss  # noqa: E402

MISSION_START_HEAD = "48173dd25f07c2f12c8e75cd55fe47c83ffbd29a"
FREEZE_PREFIX = f"{pp.EXPERIMENT_ID}:PASS2_FREEZE:"
PASS1_ID_KEYS = ("search_space_id", "protocol_id", "universe_id", "partitions_id")
SMOKE_DISCLOSURE = (
    "Before this freeze the Strategy scenario engine was smoke-executed once in memory over the real 789-candidate cohort "
    "(stdout aggregate counts only: no durable attempt, no persisted per-candidate result). The protocol thresholds and scenario "
    "rules were written before that run and are unchanged since (PASS2_PROTOCOL_ID is pinned by test); after it the gate logic "
    "was only refactored into pure helpers. The Conditional engine was NOT executed on real data before the freeze.")


class CohortRefusal(RuntimeError):
    pass


def candidate_id(kind: str, edge_id: str, source_id: str, pass1_ids: dict, protocol_id: str | None = None) -> str:
    """Result-independent: Pass-2 protocol + kind + Pass-1 edge/source identity + Pass-1 population identities only."""
    return pp.sha256_canonical({"pass2_protocol_id": protocol_id or pp.PASS2_PROTOCOL_ID, "kind": kind, "edge_id": edge_id,
                                "source_id": source_id, "pass1": {k: pass1_ids[k] for k in PASS1_ID_KEYS}})[:32]


def trial_id_of(cid: str) -> str:
    return "p2-" + cid


def sorted_root(ids) -> str:
    h = hashlib.sha256()
    for t in sorted(ids):
        h.update(t.encode("ascii") + b"\n")
    return h.hexdigest()


def strategy_cohort(edges: list[dict], cells, ledger: list[dict]) -> list[dict]:
    """Exactly every Pass-1 DISCOVERED_MODERATE StrategyEdge, cross-checked against the cell manifest and search ledger."""
    strat = [e for e in edges if e["kind"] == "STRATEGY_EDGE"]
    if any(e["edge_class"] == "DISCOVERED_STRONG" for e in strat):
        raise CohortRefusal("a Pass-1 DISCOVERED_STRONG StrategyEdge exists; the frozen cohort rule expects none")
    mods = [e for e in strat if e["edge_class"] == pp.PASS1_STRATEGY_CLASS]
    if len(mods) != pp.EXPECTED_STRATEGY_COHORT:
        raise CohortRefusal(f"strategy cohort is {len(mods)}, expected {pp.EXPECTED_STRATEGY_COHORT}: REFUSE before attempt #1")
    by_tid = {t: (ci, cfg, sym) for ci, cfg, sym, t in cells}
    ledger_mod = {r["t"] for r in ledger if r["class"] == pp.PASS1_STRATEGY_CLASS}
    if ledger_mod != {e["trial_id"] for e in mods}:
        raise CohortRefusal("search-ledger MODERATE set differs from the Edge Registry MODERATE set")
    out = []
    for e in mods:
        ci, cfg, sym = by_tid[e["trial_id"]]
        if cfg["config_id"] != e["config_id"] or sym != e["scope"] or cfg["params"] != e["params"] or \
                er.edge_id("STRATEGY_EDGE", e["trial_id"]) != e["edge_id"]:
            raise CohortRefusal(f"{e['trial_id']}: edge record disagrees with the frozen cell manifest")
        out.append({"kind": pp.KIND_STRATEGY, "candidate_id": candidate_id(pp.KIND_STRATEGY, e["edge_id"], e["trial_id"], e),
                    "edge_id": e["edge_id"], "trial_id": e["trial_id"], "config_id": e["config_id"],
                    "config_index": ci, "family": e["family"], "params": e["params"], "symbol": sym})
    return out


def conditional_cohort(edges: list[dict], factor_ledger: list[dict], conditions: list[dict], expected_ids: set) -> list[dict]:
    """Exactly every Pass-1 DISCOVERED_STRONG ConditionalEdge, cross-checked against the factor ledger and population."""
    strong = [e for e in edges if e["kind"] == "CONDITIONAL_EDGE" and e["edge_class"] == pp.PASS1_CONDITIONAL_CLASS]
    if len(strong) != pp.EXPECTED_CONDITIONAL_COHORT:
        raise CohortRefusal(f"conditional cohort is {len(strong)}, expected {pp.EXPECTED_CONDITIONAL_COHORT}: "
                            "REFUSE before attempt #1")
    if {r["factor_id"] for r in factor_ledger if r["class"] == pp.PASS1_CONDITIONAL_CLASS} != {e["factor_id"] for e in strong}:
        raise CohortRefusal("factor-ledger STRONG set differs from the Edge Registry STRONG set")
    cond_ids = {c["condition_id"] for c in conditions}
    out = []
    for e in strong:
        if e["factor_id"] not in expected_ids or e["condition_id"] not in cond_ids or \
                er.edge_id("CONDITIONAL_EDGE", e["factor_id"]) != e["edge_id"]:
            raise CohortRefusal(f"{e['factor_id']}: edge record is not a registered V3 factor of the frozen grammar")
        out.append({"kind": pp.KIND_CONDITIONAL,
                    "candidate_id": candidate_id(pp.KIND_CONDITIONAL, e["edge_id"], e["factor_id"], e),
                    "edge_id": e["edge_id"], "factor_id": e["factor_id"], "condition_id": e["condition_id"],
                    "family": e["family"], "params": e["params"], "horizon": e["horizon"]})
    return out


def candidate_identity(c: dict, pass1_ids: dict) -> dict:
    src = c.get("trial_id") or c["factor_id"]
    return {"kind": c["kind"], "edge_id": c["edge_id"], "source_id": src, "pass2_protocol_id": pp.PASS2_PROTOCOL_ID,
            "pass1": {k: pass1_ids[k] for k in PASS1_ID_KEYS}}


def cohort_manifest(strategy: list[dict], conditional: list[dict]) -> dict:
    return {"schema_version": "alpha_edge_pass2_cohort_manifest_v1", "pass2_protocol_id": pp.PASS2_PROTOCOL_ID,
            "strategy": strategy, "conditional": conditional,
            "strategy_count": len(strategy), "conditional_count": len(conditional),
            "strategy_cohort_root": sorted_root(c["candidate_id"] for c in strategy),
            "conditional_cohort_root": sorted_root(c["candidate_id"] for c in conditional),
            "robustness_attempt_count": 0}


def predeclaration(manifest: dict, binding: dict, *, starting_head: str, freeze_base_head: str, pass1_ids: dict,
                   universe: dict, ctx: dict, fence: dict) -> dict:
    protocol = pp.build_protocol()
    return {"schema_version": "alpha_edge_pass2_predeclaration_v1", "PASS2_PROTOCOL_ID": pp.PASS2_PROTOCOL_ID,
            "protocol": protocol, "starting_head": starting_head, "freeze_base_head": freeze_base_head,
            "pass1_evidence": binding, "pass1_identities": {k: pass1_ids[k] for k in PASS1_ID_KEYS},
            "partitions": {"discovery": "[2016-01-01,2024-01-01)", "forbidden": protocol["partitions"]["forbidden"]},
            "universe": {"symbol_count": universe["symbol_count"], "universe_identity": ctx["universe_identity"],
                         "data_provenance_identity": ctx["data_provenance_identity"],
                         "point_in_time_membership": universe["point_in_time_membership"],
                         "survivorship_classification": universe["survivorship_classification"],
                         "survivorship_caveat": "current-registry snapshot, NOT point-in-time; mandatory"},
            "input_fence": fence,
            "cohort": {"strategy_count": manifest["strategy_count"], "conditional_count": manifest["conditional_count"],
                       "strategy_cohort_root": manifest["strategy_cohort_root"],
                       "conditional_cohort_root": manifest["conditional_cohort_root"],
                       "cohort_manifest_sha256": pp.sha256_canonical(manifest)},
            "robustness_attempt_count": 0, "pre_freeze_engine_smoke_disclosure": SMOKE_DISCLOSURE,
            "labels": pp.LABELS, "confirmation": "NOT_RUN",
            "final_holdout": "RESERVED_UNCONSUMED", "paper": "INACTIVE", "live": "DISABLED"}


def freeze_marker(manifest: dict, binding: dict) -> dict:
    return {"pass2_protocol_id": pp.PASS2_PROTOCOL_ID, "strategy_cohort_root": manifest["strategy_cohort_root"],
            "conditional_cohort_root": manifest["conditional_cohort_root"], "strategy_count": manifest["strategy_count"],
            "conditional_count": manifest["conditional_count"],
            "pass1_binding_sha256": pp.sha256_canonical(binding)}


def require_committed(paths, repo: Path) -> str:
    """Fail closed unless every path is tracked and identical to HEAD (no staged/unstaged change). Returns HEAD."""
    repo = Path(repo)
    for p in paths:
        rel = str(Path(p).resolve().relative_to(repo.resolve()))
        if subprocess.run(["git", "ls-files", "--error-unmatch", rel], cwd=repo, capture_output=True).returncode != 0:
            raise CohortRefusal(f"{rel} is not committed: the Pass-2 freeze must be committed before attempt #1")
        if subprocess.run(["git", "diff", "--quiet", "HEAD", "--", rel], cwd=repo).returncode != 0:
            raise CohortRefusal(f"{rel} differs from HEAD: the Pass-2 freeze must be committed before attempt #1")
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()
