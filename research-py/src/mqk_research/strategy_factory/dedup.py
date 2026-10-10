"""Semantic deduplication that never reads a result.

A relationship is derived from template signatures only: (template, direction, parameters, cadence extras) against the
known index and against ideas already seen in this population. Anything the vocabulary cannot decide is reported as
UNKNOWN_NEEDS_REVIEW with advisory lexical neighbours (explicitly not proof). The complete population is always
preserved: a duplicate is labelled, never removed from the accounting.
"""

from __future__ import annotations

import re
from typing import Any, Iterable, Mapping, Sequence

from mqk_research.strategy_factory.contracts import Relationship, sha
from mqk_research.strategy_factory.known_index import SEMANTIC_NEIGHBORS, KnownEntry

OPPOSITE = {"long_flat": "short_flat", "short_flat": "long_flat", "long_only": "short_only", "short_only": "long_only"}
_STOP = frozenset("a an and the of to for in on vs versus with without by or at as from is are be test study compare "
                  "evaluate event effect rule strategy idea".split())


def tokens(text: str) -> frozenset[str]:
    return frozenset(t for t in re.findall(r"[a-z0-9]+", text.lower()) if t not in _STOP and len(t) > 1)


def jaccard(a: frozenset[str], b: frozenset[str]) -> float:
    return len(a & b) / len(a | b) if a and b else 0.0


def idea_signature(idea: Mapping[str, Any]) -> dict[str, Any] | None:
    """The comparable form of a single-template idea; None for ideas outside the recognized vocabulary."""
    tpl = idea.get("template")
    if not tpl or not tpl.get("template_id"):
        return None
    params = {k: v["value"] for k, v in tpl["params"].items() if v["value"] is not None}
    return {"template_id": tpl["template_id"], "direction": idea["direction"]["value"], "params": params,
            "complete": not tpl["missing_params"]}


def _direction_of(idea_direction: str) -> str:
    return {"long_only": "long_flat"}.get(idea_direction, idea_direction)


def classify_template(sig: Mapping[str, Any], known: Sequence[KnownEntry]) -> dict[str, Any]:
    tid, direction = sig["template_id"], _direction_of(sig["direction"])
    same = [k for k in known if k.template_id == tid]
    explicit = tuple(sorted(sig["params"].items()))

    def agrees(k: KnownEntry) -> bool:     # every parameter the idea states equals the known value
        kp = dict(k.params)
        return all(kp.get(n) == v for n, v in explicit)

    daily = [k for k in same if not k.extras]
    exact = [k for k in daily if k.direction == direction and agrees(k)] if sig["complete"] else []
    if direction == "unknown":
        rel, basis, matches = Relationship.UNKNOWN_NEEDS_REVIEW, "direction not stated: identity cannot be compared", []
    elif exact:
        rel, basis, matches = Relationship.EXACT_DUPLICATE, "same template, direction and every parameter", exact
    elif sig["complete"] and (mir := [k for k in daily if k.direction == OPPOSITE.get(direction) and agrees(k)]):
        rel, basis, matches = Relationship.MIRROR, "same template and parameters, opposite direction", mir
    elif sig["complete"] and (cad := [k for k in same if k.extras and k.direction == direction and agrees(k)]):
        rel, basis, matches = Relationship.SEMANTIC_VARIANT, "same template and parameters at a different cadence", cad
    elif same:
        rel, basis, matches = Relationship.PARAMETER_VARIANT, (
            "same template; parameters differ" if sig["complete"] else "same template; parameters not fully stated"), same
    else:
        near = [k for k in known if frozenset((tid, k.template_id)) in SEMANTIC_NEIGHBORS]
        rel, basis, matches = ((Relationship.SEMANTIC_VARIANT, "documented neighbouring template", near) if near
                               else (Relationship.UNKNOWN_NEEDS_REVIEW, "recognized template with no known counterpart", []))
    tested = sorted({b for k in same for b in k.tested_in})
    return {"relationship": rel.value, "basis": basis, "matches": sorted(k.known_id for k in matches)[:12],
            "match_count": len(matches), "same_template_known": len(same), "previously_tested_in": tested}


def dedup_population(ideas: Sequence[Mapping[str, Any]], known: Sequence[KnownEntry]) -> dict[str, dict[str, Any]]:
    """intake_id -> relationship record for the whole submitted population, in the order given (stable)."""
    out: dict[str, dict[str, Any]] = {}
    seen_sig: dict[str, str] = {}
    seen_tok: list[tuple[str, frozenset[str]]] = []
    for idea in sorted(ideas, key=lambda i: i["intake_id"]):
        sig = idea_signature(idea)
        if sig is not None:
            rec = classify_template(sig, known)
            if sig["complete"]:
                key = sha({"t": sig["template_id"], "d": _direction_of(sig["direction"]), "p": sig["params"]})
                first = seen_sig.setdefault(key, idea["intake_id"])
                if first != idea["intake_id"] and rec["relationship"] != Relationship.EXACT_DUPLICATE.value:
                    rec = {**rec, "relationship": Relationship.EXACT_DUPLICATE.value,
                           "basis": "same template, direction and parameters as an earlier submitted idea",
                           "matches": [first], "match_count": 1}
        elif idea["kind"] == "COMPOSITE_RULE":
            comps = idea["template"]["components"]
            have = all(any(k.template_id == c for k in known) for c in comps)
            rec = {"relationship": (Relationship.COMPOSITE_OF_EXISTING if have else Relationship.UNKNOWN_NEEDS_REVIEW).value,
                   "basis": f"components {comps}", "matches": [], "match_count": 0,
                   "same_template_known": 0, "previously_tested_in": []}
        else:
            tok = tokens(idea["source_text"]["title"])
            near = sorted(((round(jaccard(tok, t), 3), i) for i, t in seen_tok if jaccard(tok, t) >= 0.6), reverse=True)[:5]
            rec = {"relationship": Relationship.UNKNOWN_NEEDS_REVIEW.value,
                   "basis": "no recognized rule template; paraphrase cannot be ruled in or out", "matches": [i for _, i in near],
                   "match_count": len(near), "same_template_known": 0, "previously_tested_in": [],
                   "advisory_lexical_neighbours": [{"intake_id": i, "jaccard": j} for j, i in near]}
        seen_tok.append((idea["intake_id"], tokens(idea["source_text"]["title"])))
        out[idea["intake_id"]] = rec
    return out


OPERATOR_RELATIONSHIPS = frozenset(r.value for r in Relationship) - {Relationship.EXACT_DUPLICATE.value}


def apply_novelty_review(rec: Mapping[str, Any], review: Mapping[str, Any]) -> dict[str, Any]:
    """An operator may resolve ONLY an UNKNOWN_NEEDS_REVIEW relationship, with a named rationale and reference."""
    if rec["relationship"] != Relationship.UNKNOWN_NEEDS_REVIEW.value:
        raise ValueError("a novelty review resolves only UNKNOWN_NEEDS_REVIEW")
    if review.get("relationship") not in OPERATOR_RELATIONSHIPS - {Relationship.UNKNOWN_NEEDS_REVIEW.value} \
            or not review.get("decided_by") or not review.get("rationale") or not review.get("decision_ref"):
        raise ValueError("a novelty review needs a resolving relationship, decided_by, rationale and decision_ref")
    return {**rec, "relationship": review["relationship"], "basis": f"operator review {review['decision_ref']}: {review['rationale']}",
            "operator_review": {k: review[k] for k in ("relationship", "decided_by", "rationale", "decision_ref")}}
