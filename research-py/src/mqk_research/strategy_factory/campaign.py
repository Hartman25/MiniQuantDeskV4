"""Campaign specification -> predeclaration compiler.

A campaign spec is operator-authored and names WHAT to test (population sources, symbols, data pins, partition); it
cannot name economics. Costs, sizing, benchmark, robustness and stress come only from a previously frozen protocol
profile (the Batch-03 blocks, pinned by content hash), so the Factory never invents or silently reuses a policy: the
operator's stage authorization is bound to the exact declaration this module emits.

The compiler is pure (spec, ideas, repo files in; declaration + trial population out). It reads no price, writes no
registry and runs no stage. The complete trial population is fixed here, before any registration or economic attempt.
"""

from __future__ import annotations

import copy
import hashlib
import itertools
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

from mqk_research.strategy_factory.contracts import sha
from mqk_research.strategy_factory.dedup import classify_template
from mqk_research.strategy_factory.known_index import build_index
from mqk_research.strategy_factory.templates import (
    CARD_BY_ID, GRAMMAR_PREFIX, TEMPLATES, grammar_strategy_name, parse_grammar_name, required_history_bars)

SPEC_SCHEMA = "factory_campaign_spec_v1"
DECL_SCHEMA = "factory_campaign_declaration_v1"
EXPERIMENTS_REL = Path("research-py/experiments/m1_native_trend_campaign")
PROFILES = {"m1_batch03_v1": ("PREDECLARED_BATCH_03.json", "b1f51fdfb5f8d569e3f4104ede7492e567c35aa115f657ee41c7fa62bdc73c76")}
PROFILE_BLOCKS = ("calendar", "economic_protocol", "native_backtest", "capital_sizing", "benchmark", "robustness",
                  "scanner_review", "promotion_policy", "execution_fidelity")
GRADES = {
    "SYNTHETIC_DIAGNOSTIC": "Synthetic diagnostic data: proves the software path only. It is not market evidence, cannot reach "
                            "Promotion, and must never be reported as research readiness.",
    "EXPOSED_DEVELOPMENT": "Declared-exposed development data: shows only whether the registered hypothesis clears the existing "
                           "economic gates on exposed data. It is not independent confirmation.",
}
FORBIDDEN_LABELS = ["INDEPENDENTLY_CONFIRMED", "OUT_OF_SAMPLE", "CONFIRMED", "VALIDATED"]
_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{2,63}$")
_SYMBOL = re.compile(r"^[A-Z][A-Z0-9.]{0,9}$")
MAX_TRIALS_HARD = 500


class CampaignError(Exception):
    """The campaign cannot be compiled; nothing is partially declared."""


def canonical(obj: Any) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def declaration_identity(decl: Mapping[str, Any]) -> str:
    """Identical to experiments stage_authorization.declaration_identity: everything except `execution_gate`."""
    return hashlib.sha256(canonical({k: v for k, v in decl.items() if k != "execution_gate"}).encode("utf-8")).hexdigest()


def load_profile(repo_root: Path, profile_id: str) -> dict[str, Any]:
    if profile_id not in PROFILES:
        raise CampaignError(f"unknown protocol profile {profile_id!r}; known: {sorted(PROFILES)}")
    name, pin = PROFILES[profile_id]
    path = Path(repo_root) / EXPERIMENTS_REL / name
    try:
        decl = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise CampaignError(f"protocol profile source unreadable: {exc}") from exc
    if hashlib.sha256(canonical(decl).encode("utf-8")).hexdigest() != pin:
        raise CampaignError(f"protocol profile {profile_id!r} no longer matches its pinned content hash")
    return {k: copy.deepcopy(decl[k]) for k in PROFILE_BLOCKS} | {"_hypothesis_templates": decl["hypotheses"], "_pin": pin}


def historical_hypotheses(repo_root: Path) -> dict[str, dict[str, Any]]:
    """strategy_id -> the most recent frozen hypothesis object (for native engines whose requirements are already declared)."""
    out: dict[str, dict[str, Any]] = {}
    for path in sorted((Path(repo_root) / EXPERIMENTS_REL).glob("PREDECLARED_*.json")):
        decl = json.loads(path.read_text(encoding="utf-8"))
        for h in decl.get("hypotheses", []):
            if isinstance(h, dict) and h.get("strategy_id") and isinstance(h.get("required_history_bars"), int):
                out[h["strategy_id"]] = h
    return out


# ------------------------------------------------------------------------------------------------ spec validation
def validate_spec(spec: Mapping[str, Any]) -> None:
    if spec.get("schema") != SPEC_SCHEMA:
        raise CampaignError(f"spec schema must be {SPEC_SCHEMA!r}")
    if not _ID.match(str(spec.get("campaign_id", ""))):
        raise CampaignError("campaign_id must be 3-64 characters of [A-Za-z0-9_.-]")
    if spec.get("evidence_grade") not in GRADES:
        raise CampaignError(f"evidence_grade must be one of {sorted(GRADES)}")
    if spec.get("protocol_profile") not in PROFILES:
        raise CampaignError(f"protocol_profile must be one of {sorted(PROFILES)}")
    if not re.match(r"^\d{4}-\d{2}-\d{2}$", str(spec.get("predeclared_utc_date", ""))):
        raise CampaignError("predeclared_utc_date must be an explicit YYYY-MM-DD (no wall clock enters an identity)")
    pop = spec.get("population")
    if not isinstance(pop, dict) or not pop.get("sources") or not isinstance(pop["sources"], list):
        raise CampaignError("population.sources must be a non-empty list")
    symbols = pop.get("symbols")
    if not isinstance(symbols, list) or not symbols or len(set(symbols)) != len(symbols) or not all(isinstance(s, str) and _SYMBOL.match(s) for s in symbols):
        raise CampaignError("population.symbols must be a non-empty list of unique uppercase tickers")
    mt = pop.get("max_trials")
    if type(mt) is not int or not 1 <= mt <= MAX_TRIALS_HARD:
        raise CampaignError(f"population.max_trials must be an integer in [1, {MAX_TRIALS_HARD}]")
    data, part = spec.get("data"), spec.get("partition")
    if not isinstance(data, dict) or data.get("mode") != "reuse":
        raise CampaignError("data.mode must be 'reuse' (a provider fetch is a separate authorized operator action)")
    for k in ("reuse_from", "expected_artifact_sha256", "expected_row_count", "expected_canonical_semantic_bars_hash",
              "start_utc", "end_utc", "asof"):
        if k not in data:
            raise CampaignError(f"data.{k} is required")
    for k in ("evaluation_start_utc", "test_months", "holdout_months", "expected_folds", "holdout_boundary"):
        if not isinstance(part, dict) or k not in part:
            raise CampaignError(f"partition.{k} is required (a graded declaration must name a fixed reserved boundary)")
    for src in pop["sources"]:
        if not isinstance(src, dict) or src.get("kind") not in ("native", "grammar_grid", "admitted_ideas"):
            raise CampaignError("each population source needs kind native | grammar_grid | admitted_ideas")


# ------------------------------------------------------------------------------------------------ population
@dataclass(frozen=True)
class StrategySlot:
    strategy_name: str
    template_id: str | None
    params: tuple[tuple[str, int], ...]
    origin: str                      # native | grammar_v1
    provenance: tuple[str, ...]      # which population sources asked for it (several sources never duplicate a slot)
    intake_ids: tuple[str, ...] = ()


def expand_sources(spec: Mapping[str, Any], ideas: Mapping[str, Mapping[str, Any]], grammar_available: bool) -> tuple[list[StrategySlot], dict[str, Any]]:
    wanted: dict[str, dict[str, Any]] = {}
    report: dict[str, Any] = {"sources": [], "excluded": []}
    for n, src in enumerate(spec["population"]["sources"]):
        label = f"{n}:{src['kind']}"
        names: list[tuple[str, str | None, dict[str, int] | None, str, str | None]] = []
        if src["kind"] == "native":
            for sid in src.get("strategy_ids", []):
                if sid not in CARD_BY_ID or CARD_BY_ID[sid].template_id == "legacy_engine" and sid not in src.get("allow_legacy", []):
                    raise CampaignError(f"native strategy {sid!r} has no semantic card (or is a legacy engine)")
                names.append((sid, None, None, "native", None))
        elif src["kind"] == "grammar_grid":
            if not grammar_available:
                raise CampaignError("grammar_v1 is not available in this build; a grammar_grid source is refused")
            tid, grid = src.get("template"), src.get("grid")
            tpl = TEMPLATES.get(tid)
            if tpl is None or not tpl.grammar_v1 or not isinstance(grid, dict) or set(grid) != {p.name for p in tpl.params}:
                raise CampaignError(f"grammar_grid needs an executable template and a grid over exactly its parameters ({tid!r})")
            axes = []
            for p in tpl.params:
                vals = grid[p.name]
                if not isinstance(vals, list) or not vals or len(set(vals)) != len(vals):
                    raise CampaignError(f"grid axis {p.name} must be a non-empty list of unique integers")
                axes.append(sorted(vals))
            declared = 1
            for a in axes:
                declared *= len(a)
            realized = 0
            for combo in itertools.product(*axes):
                params = dict(zip((p.name for p in tpl.params), combo))
                try:
                    tpl.validate(params)
                except ValueError as exc:
                    report["excluded"].append({"source": label, "params": params, "reason": str(exc)})   # accounted, never silent
                    continue
                realized += 1
                names.append((grammar_strategy_name(tid, params), tid, params, "grammar_v1", None))
            if realized + sum(1 for e in report["excluded"] if e["source"] == label) != declared:
                raise CampaignError("grid accounting mismatch")
        else:
            for iid in src.get("intake_ids", []):
                rec = ideas.get(iid)
                if rec is None:
                    raise CampaignError(f"admitted_ideas source names unknown idea {iid!r}")
                ok = rec["disposition"] in ("ADMITTED_NATIVE", "ADMITTED_GRAMMAR") or \
                    (rec["disposition"] == "DUPLICATE_OF_KNOWN" and src.get("allow_reevaluation_of_known") is True
                     and (rec.get("execution_path") or {}).get("kind") == "native")
                if not ok or not rec.get("execution_path"):
                    raise CampaignError(f"idea {iid} is {rec['disposition']}; only an admitted idea can enter a population")
                path = rec["execution_path"]
                name = path["strategy_id"] if path["kind"] == "native" else path["strategy_name"]
                names.append((name, None, None, path["kind"], iid))
        for name, tid, params, origin, iid in names:
            slot = wanted.setdefault(name, {"origin": origin, "prov": [], "ideas": [], "tid": tid, "params": params})
            slot["prov"].append(label)
            if iid:
                slot["ideas"].append(iid)
        report["sources"].append({"label": label, "kind": src["kind"], "strategies": sorted({x[0] for x in names})})
    slots = []
    for name in sorted(wanted):
        w = wanted[name]
        tid, params = w["tid"], w["params"]
        if w["origin"] == "grammar_v1" and tid is None:
            tid, params = parse_grammar_name(name)
        slots.append(StrategySlot(name, tid, tuple(sorted((params or {}).items())), w["origin"], tuple(sorted(set(w["prov"]))),
                                  tuple(sorted(set(w["ideas"])))))
    return slots, report


def _relationship(slot: StrategySlot, known: Sequence[Any]) -> dict[str, Any]:
    """Disclosure only (never a filter): how the slot relates to everything already known or searched."""
    if slot.template_id is None:
        card = CARD_BY_ID.get(slot.strategy_name)
        if card is None:
            return {"relationship": "UNKNOWN_NEEDS_REVIEW", "basis": "no semantic card"}
        sig = {"template_id": card.template_id, "direction": card.direction, "params": dict(card.params), "complete": True}
    else:
        sig = {"template_id": slot.template_id, "direction": "long_flat", "params": dict(slot.params), "complete": True}
    r = classify_template(sig, [k for k in known if not (slot.template_id is None and k.known_id == f"native:{slot.strategy_name}")])
    return {k: r[k] for k in ("relationship", "basis", "matches", "same_template_known", "previously_tested_in")}


def _hypothesis(slot: StrategySlot, campaign_id: str, label: str, historical: Mapping[str, Mapping[str, Any]]) -> dict[str, Any]:
    hid = f"fc_{campaign_id}_{slot.strategy_name}"
    if slot.origin == "native":
        base = historical.get(slot.strategy_name)
        if base is None:
            raise CampaignError(f"native strategy {slot.strategy_name!r} has no frozen hypothesis declaring its required history; "
                                "it cannot be predeclared without a native-identity check")
        h = copy.deepcopy(dict(base))
        h.update(hypothesis_id=hid, hypothesis_label=label)
        return h
    params = dict(slot.params)
    return {
        "hypothesis_id": hid, "hypothesis_label": label, "strategy_id": slot.strategy_name,
        "economic_rationale": f"grammar_v1 {slot.template_id} {params}: a stateless long/flat rule from the closed template vocabulary; "
                              "parameters are predeclared coordinates of a bounded population, not tuned values.",
        "timeframe_secs": 86400, "required_history_bars": required_history_bars(slot.template_id, params),
        "direction": "long_only_flat", "rule": "see strategy_id (the name is the complete specification)",
        "decision_kind": "daily_state", "parameters": params, "calendar_contract_bound": True,
        "restart_recovery": "BoundedHistoryReconstructible", "no_variants": "the population is the predeclared grid only",
    }


def check_data_authority(spec: Mapping[str, Any]) -> None:
    """Synthetic data may never carry a market-evidence grade, and official data may not hide as synthetic."""
    manifest = Path(spec["data"]["reuse_from"]) / "research_bars_provenance.json"
    try:
        m = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise CampaignError(f"verified bars manifest unreadable at {manifest}: {exc}") from exc
    authority = (m.get("source_attestation") or {}).get("source_authority")
    official = authority == "official_provider"
    if official != (spec["evidence_grade"] != "SYNTHETIC_DIAGNOSTIC"):
        raise CampaignError(f"data source_authority={authority!r} is inconsistent with evidence_grade {spec['evidence_grade']}: "
                            "synthetic data can never be graded as market evidence, nor official data as synthetic")
    for k, mk in (("expected_artifact_sha256", "artifact_sha256"), ("expected_row_count", "row_count"),
                  ("expected_canonical_semantic_bars_hash", "canonical_semantic_bars_hash")):
        if spec["data"][k] != m.get(mk):
            raise CampaignError(f"data.{k} does not match the bars manifest")


@dataclass(frozen=True)
class Compiled:
    declaration: dict[str, Any]
    trials: list[dict[str, Any]]
    population_report: dict[str, Any]
    declaration_sha256: str


def compile_campaign(spec: Mapping[str, Any], *, repo_root: Path, run_root: Path, ideas: Mapping[str, Mapping[str, Any]],
                     grammar_available: bool, prior_search: Mapping[str, Any] | None = None) -> Compiled:
    validate_spec(spec)
    cid = spec["campaign_id"]
    check_data_authority(spec)
    profile = load_profile(repo_root, spec["protocol_profile"])
    historical = historical_hypotheses(repo_root)
    slots, report = expand_sources(spec, ideas, grammar_available)
    known = build_index(repo_root)
    symbols = sorted(spec["population"]["symbols"])
    trial_count = len(slots) * len(symbols)
    if not slots:
        raise CampaignError("the population is empty")
    if trial_count > spec["population"]["max_trials"]:
        raise CampaignError(f"population has {trial_count} trials, above the predeclared max_trials {spec['population']['max_trials']}")
    labels = {s.strategy_name: f"H{n + 1:03d}" for n, s in enumerate(slots)}
    hypotheses = [_hypothesis(s, cid, labels[s.strategy_name], historical) for s in slots]
    trials = [{"order": n + 1, "hypothesis_label": labels[s.strategy_name], "strategy_id": s.strategy_name, "symbol": sym}
              for n, (s, sym) in enumerate((s, sym) for s in slots for sym in symbols)]
    run_dir = (Path(run_root) / cid).resolve()
    part = copy.deepcopy(spec["partition"])
    part.setdefault("holdout_rule", "the final reserved window is RESERVED and not consumed")
    d = spec["data"]
    decl: dict[str, Any] = {
        "schema": DECL_SCHEMA, "batch_id": cid, "mission_id": spec.get("mission_ref", "V4-STRATEGY-FACTORY-RESEARCH-BACKTEST-FULL-COMPLETION-01"),
        "predeclared_utc_date": spec["predeclared_utc_date"],
        "wave_classification": "STRATEGY_FACTORY_CAMPAIGN",
        "evidence_grade": {"grade": spec["evidence_grade"], "independent_confirmation": False, "forbidden_labels": FORBIDDEN_LABELS,
                           "statement": GRADES[spec["evidence_grade"]]},
        "execution_gate": {"status": "FACTORY_PREDECLARED_NOT_RELEASED", "executable": False,
                           "blocker": "OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED",
                           "rule": "No stage runs until the operator releases the gate AND supplies a signed stage authorization bound to this "
                                   "declaration identity. The Factory never mints either."},
        "run_dir": str(run_dir).replace("\\", "/"),
        "declaration_selection": "run_batch.py reads this file when MQK_M1_BATCH_DECLARATION names it (absolute path).",
        "calendar": profile["calendar"],
        "hypotheses": hypotheses,
        "universe": {"selection_rule": spec["population"].get("selection_rule", "fixed ex-ante list declared by the campaign spec"),
                     "symbols": symbols, "universe_mode": "fixed_ex_ante", "point_in_time_membership": False,
                     "survivorship_note": "A fixed ex-ante symbol list is not point-in-time clean; survivorship is disclosed, not corrected.",
                     "max_trials": trial_count, "additional_candidate_after_predeclaration": "forbidden", "trials": trials},
        "experiment": {"real_experiment_id": f"FACTORY-{cid}",
                       "registry_db_relative_path": str(run_dir / "registry" / "research.sqlite3").replace("\\", "/"),
                       "trial_per_hypothesis_symbol": True, "judge_scope": "whole experiment, never a subset",
                       "population_policy": "ONE experiment id holds every predeclared trial; the judge sees the whole population.",
                       "all_trials_registered_before_first_economic_evaluation": True},
        "data": {"path": "reuse_verified_data", "feed": d.get("feed", "reused"), "timeframe": "1Day",
                 "timeframe_identity": "canonical_semantic_v1", "adjustment": d.get("adjustment", "all"),
                 "start_utc": d["start_utc"], "end_utc": d["end_utc"], "asof": d["asof"], "completed_bars_only": True,
                 "reuse_verified_data_from": {"run_dir": str(d["reuse_from"]).replace("\\", "/"),
                                              "expected_artifact_sha256": d["expected_artifact_sha256"],
                                              "expected_row_count": d["expected_row_count"],
                                              "expected_canonical_semantic_bars_hash": d["expected_canonical_semantic_bars_hash"],
                                              **({"expected_source_attestation_id": d["expected_source_attestation_id"]}
                                                 if d.get("expected_source_attestation_id") else {})}},
        "partition": part,
        **{k: profile[k] for k in PROFILE_BLOCKS},
        "holdout": {"status": "reserved_not_evaluated", "rule": "The reserved window is never read, fetched or consumed by a Factory campaign."},
        "factory": {"schema": DECL_SCHEMA, "spec_sha256": sha(spec), "protocol_profile": spec["protocol_profile"],
                    "protocol_profile_pin": profile["_pin"], "population_report": report,
                    "strategies": [{"strategy_name": s.strategy_name, "origin": s.origin, "template_id": s.template_id,
                                    "params": dict(s.params), "population_sources": list(s.provenance), "intake_ids": list(s.intake_ids),
                                    "relationship_to_known": _relationship(s, known)}
                                   for s in slots],
                    "prior_search_disclosure": dict(prior_search or {}),
                    "promotion_eligible": spec["evidence_grade"] != "SYNTHETIC_DIAGNOSTIC",
                    "paper_live": "NOT_AUTHORIZED: a Factory campaign grants no Promotion, Paper or Live authority"},
    }
    trial_rows = [{"trial_key": f"{t['strategy_id']}/{t['symbol']}", "strategy_name": t["strategy_id"], "symbol": t["symbol"],
                   "source_intake_id": (next(s for s in slots if s.strategy_name == t["strategy_id"]).intake_ids or (None,))[0],
                   "relationship": None} for t in trials]
    return Compiled(decl, trial_rows, report, declaration_identity(decl))
