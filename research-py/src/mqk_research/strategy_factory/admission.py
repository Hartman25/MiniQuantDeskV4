"""Executable-compatibility admission: which idea may run, on which verified path, and why every other idea may not.

An idea is admitted only when ALL of: it is a recognized single-template rule, every parameter is stated (source rule or
recorded operator decision), asset class and direction are inside the operational scope, no blocker remains, it is not
an exact duplicate of something already known, and a verified executable path exists (an existing native identity, or
the grammar engine for a grammar_v1 template). Nothing is admitted because it has a fingerprint, compiles, or sounds
plausible. The disposition is explicit for every entry; nothing is silently dropped.
"""

from __future__ import annotations

from typing import Any, Mapping

from mqk_research.strategy_factory.contracts import Admission, Disposition, Relationship
from mqk_research.strategy_factory.templates import CARD_BY_ID, NATIVE_CARDS, TEMPLATES, grammar_strategy_name

OPERATIONAL_ASSET = "equity"
OPERATIONAL_DIRECTIONS = ("long_flat", "long_only")


def native_path_for(template_id: str, params: Mapping[str, int], direction: str) -> str | None:
    for c in NATIVE_CARDS:
        if c.template_id == template_id and c.direction == "long_flat" and dict(c.params) == dict(params) \
                and direction in OPERATIONAL_DIRECTIONS:
            return c.strategy_id
    return None


def decide(idea: Mapping[str, Any], dedup: Mapping[str, Any], *, grammar_available: bool) -> dict[str, Any]:
    """Return {admission, disposition, execution_path, reasons}. Pure; result-independent."""
    reasons: list[str] = []
    blockers = idea["blockers"]
    kind = idea["kind"]

    def out(adm, disp, path=None):
        return {"admission": adm.value, "disposition": disp.value, "execution_path": path, "reasons": reasons}

    if kind == "GOVERNANCE_CONTROL":
        reasons.append("control/guardrail row, not a tradable idea")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.GOVERNANCE_CONTROL)
    if kind == "BENCHMARK":
        reasons.append("comparison baseline (buy-and-hold / rebalance benchmark), not a candidate trial")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.BENCHMARK_NOT_STRATEGY)
    if "F" in blockers:
        reasons.append("asset class, derivative structure or rates/FX carry outside the operational equity/ETF scope")
        return out(Admission.REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS, Disposition.DEFERRED_ASSET_CLASS)
    if "L" in blockers or "D" in blockers:
        reasons.append("requires " + "/".join(t for t in idea["required_data"] if t in
                                              ("ML", "NEWS_TEXT", "FUNDAMENTAL", "ORDER_FLOW", "EARNINGS_CALENDAR",
                                               "PIT_UNIVERSE", "INTRADAY")) + " data or a model framework MQD does not hold")
        return out(Admission.REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS, Disposition.UNSUPPORTED_DATA)
    if "P" in blockers:
        reasons.append("needs a multi-symbol ranking/portfolio engine")
        return out(Admission.REQUIRES_NEW_IMPLEMENTATION, Disposition.NEEDS_IMPLEMENTATION)
    tpl = idea.get("template")
    if kind == "DIAGNOSTIC_QUESTION":
        reasons.append("phenomenon/event-study question; route to factor diagnostics, not a strategy trial")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.DIAGNOSTIC_NOT_STRATEGY)
    if "S" in blockers or "X" in blockers:
        reasons.append("direction/short or execution policy requires an operator decision (no short/borrow authority in M1)")
        return out(Admission.REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS, Disposition.NEEDS_OPERATOR_POLICY)
    if kind == "UNRECOGNIZED":
        reasons.append("nothing computable can be recovered from the text")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.REJECTED_UNDERSPECIFIED)
    if kind in ("RULE_TEXT_UNMAPPED", "COMPOSITE_RULE") or tpl is None or not tpl.get("template_id"):
        reasons.append("a rule is described in prose but does not map to a recognized template")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.NEEDS_FORMALIZATION)
    if tpl["missing_params"]:
        reasons.append("unstated parameters: " + ", ".join(tpl["missing_params"]) + " (not defaulted; operator decision required)")
        return out(Admission.REJECTED_INSUFFICIENTLY_SPECIFIED, Disposition.NEEDS_FORMALIZATION)
    if idea["asset_class"]["value"] != OPERATIONAL_ASSET or idea["direction"]["value"] not in OPERATIONAL_DIRECTIONS:
        reasons.append("asset class or direction not stated as operational equity long/flat")
        return out(Admission.REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS, Disposition.NEEDS_OPERATOR_POLICY)
    params = {n: v["value"] for n, v in tpl["params"].items()}
    spec = TEMPLATES[tpl["template_id"]]
    spec.validate(params)
    native = native_path_for(tpl["template_id"], params, idea["direction"]["value"])
    rel = dedup["relationship"]
    if rel == Relationship.EXACT_DUPLICATE.value:
        reasons.append("exact duplicate of " + ", ".join(dedup["matches"][:3])
                       + (f" (already evaluated in {dedup['previously_tested_in']})" if dedup["previously_tested_in"] else ""))
        return out(Admission.NATIVE_EXECUTABLE if native else Admission.REQUIRES_NEW_IMPLEMENTATION,
                   Disposition.DUPLICATE_OF_KNOWN, {"kind": "native", "strategy_id": native} if native else None)
    if native:
        reasons.append("existing native identity")
        return out(Admission.NATIVE_EXECUTABLE, Disposition.ADMITTED_NATIVE, {"kind": "native", "strategy_id": native})
    if spec.grammar_v1 and grammar_available:
        name = grammar_strategy_name(tpl["template_id"], params)
        reasons.append(f"expressible in the verified grammar engine ({rel.lower()})")
        return out(Admission.GRAMMAR_EXPRESSIBLE, Disposition.ADMITTED_GRAMMAR, {"kind": "grammar_v1", "strategy_name": name})
    reasons.append("recognized and fully specified, but no verified executable exists"
                   + (" (stateful template needs durable-restart implementation)" if spec.stateful else ""))
    return out(Admission.REQUIRES_NEW_IMPLEMENTATION, Disposition.NEEDS_IMPLEMENTATION)
