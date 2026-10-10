"""Deterministic formalization of one catalog entry into a typed StrategyIdea record (untrusted text in, typed fields out).

Every field carries a class: EXPLICIT_SOURCE_RULE only when the source states it, INFERRED_RULE only through a named
alias or an operator decision, UNDERSPECIFIED when the source gestures at it without computable content, UNKNOWN when it
says nothing. Nothing is defaulted. No price, return, rank or result is read. An intake record is not a hypothesis,
trial, Promotion, Paper or Live authority.
"""

from __future__ import annotations

import re
from typing import Any, Mapping

from mqk_research.strategy_factory import SCHEMA_VERSION
from mqk_research.strategy_factory.contracts import FieldClass, intake_id, sha
from mqk_research.strategy_factory.templates import TEMPLATES, Match, extract

IDEA_SCHEMA = "strategy_factory_idea_v1"

_DIAGNOSTIC = re.compile(r"\b(compare|test|study|event study|measure|evaluate|observe|assess|check|classify|does|do|is there|"
                         r"can|whether|estimate|screen|examine|investigate|determine|quantify)\b|\?", re.I)
_RULE = re.compile(r"\b(enter|exit|hold|buy|sell|go long|go short|trade|rebalance|long|short|close the position)\b", re.I)

_BENCHMARK = re.compile(r"\s*(?:buy[- ]and[- ]hold|equal[- ]weight\b.*\bbasket|fixed periodic rebalanc|benchmark)", re.I)

_DATA_TAGS: tuple[tuple[str, str], ...] = (
    ("INTRADAY", r"intraday|minute|\b1m\b|\b5m\b|vwap|opening range|first \d+ minutes|\borb\b"),
    ("ORDER_FLOW", r"tick|order book|order[- ]flow|footprint|volume profile|volume delta|market depth|funding|value[- ]area|point of control"),
    ("OPTIONS", r"\boptions?\b|implied vol|\biv\b|straddle|condor|credit spread|covered call|iv surface|put writing|\bwheel\b"),
    ("FUNDAMENTAL", r"fundamental|valuation|\bpe\b|\broe\b|roic|solvency|balance sheet|profitability|ev-ebitda|book value"),
    ("NEWS_TEXT", r"\bnews\b|headline|sentiment|\bnlp\b|embedding|filings?\b|press release|text\b"),
    ("EARNINGS_CALENDAR", r"earnings (?:calendar|date|announcement)|post-earnings|pre-earnings"),
    ("PIT_UNIVERSE", r"point-in-time|\bpit\b|historical membership|constituent"),
    ("MULTI_SYMBOL", r"\brank|rotation|top-?n|bottom-?n|\bpairs?\b|spread z|basket|vs\.? spy|versus spy|\bvs spy|relative strength|"
                     r"cross-sectional|sector (?:etf|rank|leader|rotation|neutral)|market[- ]neutral|\+ ?spy|two symbols|multi-symbol"),
    ("ML", r"machine learning|classifier|probability model|regime cluster|bayesian|neural|supervised|\bml\b|feature pipeline|"
           r"frozen model|walk-forward validation"),
    ("RATES_FX_CARRY", r"carry|swap|forwards?\b|interest differential|central[- ]bank|rollover"),
)
_NON_EQUITY = (("OPTIONS", "option"), ("FX", r"\bfx\b|forex|g10|currenc|cross-pairs"), ("CRYPTO", r"crypto|bitcoin|perpetual|funding"),
               ("FUTURES", r"futures|\bes\b|\bnq\b|commodit"))


def _text(*parts: str) -> str:
    return " ".join(p for p in (s.strip() for s in parts) if p)


def _fv(value: Any, cls: FieldClass, basis: str = "") -> dict[str, Any]:
    return {"value": value, "class": cls.value, "basis": basis}


def parse_direction(raw: str) -> tuple[str, FieldClass]:
    r = " ".join(raw.lower().replace("-", " ").split())
    if not r:
        return "unknown", FieldClass.UNKNOWN
    if re.search(r"premium|volatility|vol\b", r):
        return "derivative_structure", FieldClass.EXPLICIT_SOURCE_RULE
    if re.fullmatch(r"long ?/ ?flat|long flat|long or flat", r):
        return "long_flat", FieldClass.EXPLICIT_SOURCE_RULE
    if re.fullmatch(r"long ?/ ?short|long short|long short neutral", r):
        return "long_short", FieldClass.EXPLICIT_SOURCE_RULE
    if r == "long":
        return "long_only", FieldClass.EXPLICIT_SOURCE_RULE
    if r == "short":
        return "short_only", FieldClass.EXPLICIT_SOURCE_RULE
    return "unknown", FieldClass.UNDERSPECIFIED


def parse_asset_class(raw: str) -> tuple[str, FieldClass]:
    r = raw.lower()
    if not r.strip():
        return "unknown", FieldClass.UNKNOWN
    for tag, pat in _NON_EQUITY:
        if re.search(pat, r):
            return tag.lower(), FieldClass.EXPLICIT_SOURCE_RULE
    if re.search(r"stock|etf|equit|letf|shares", r):
        return "equity", FieldClass.EXPLICIT_SOURCE_RULE
    return "unknown", FieldClass.UNDERSPECIFIED


def data_tags(*texts: str) -> list[str]:
    t = " ".join(texts).lower()
    return sorted(tag for tag, pat in _DATA_TAGS if re.search(pat, t))


def blockers_for(*, asset_class: str, direction: str, tags: list[str], composite_unmapped: bool) -> str:
    """Blocker letters in the vocabulary of experiments/external_idea_intake/disposition.py."""
    b = set()
    if asset_class in ("options", "fx", "crypto", "futures") or "OPTIONS" in tags or "RATES_FX_CARRY" in tags \
            or direction == "derivative_structure":
        b.add("F")
    if "ML" in tags or "NEWS_TEXT" in tags:
        b.add("L")
    if {"FUNDAMENTAL", "ORDER_FLOW", "EARNINGS_CALENDAR", "PIT_UNIVERSE", "INTRADAY"} & set(tags):
        b.add("D")
    if "MULTI_SYMBOL" in tags:
        b.add("P")
    if direction in ("long_short", "short_only"):
        b.add("S")
    if composite_unmapped:
        b.add("U")
    return "".join(c for c in "FLDPSXU" if c in b)


def formalize_entry(entry: Mapping[str, Any], ledger: Mapping[str, Any]) -> dict[str, Any]:
    """One catalog ledger entry -> one idea record. Pure and deterministic."""
    f = entry["fields"]
    family = ledger["catalog_family"]
    title, hyp, mech, rule = f.get("title", ""), f.get("hypothesis", ""), f.get("mechanism", ""), f.get("rule_text", "")
    corpus = _text(title, hyp, rule)
    matches = extract(corpus)
    direction, dcls = parse_direction(f.get("direction", ""))
    asset, acls = parse_asset_class(f.get("assets", ""))
    # The catalog's own data column is the declared requirement. Free-text keywords decide only when no data column exists
    # (a diagnostic that merely MENTIONS news is not thereby a news-dependent strategy).
    declared = f.get("data_needed", "")
    tags = data_tags(declared) if declared.strip() else data_tags(corpus)
    is_control = entry["entry_kind"] == "control"
    has_diag = bool(_DIAGNOSTIC.search(_text(hyp, rule) or title))
    has_rule = bool(_RULE.search(rule))
    if is_control:
        kind = "GOVERNANCE_CONTROL"
    elif _BENCHMARK.match(title) and not matches:
        kind = "BENCHMARK"
    elif len(matches) > 1:
        kind = "COMPOSITE_RULE"
    elif len(matches) == 1:
        kind = "RULE_STRATEGY"
    elif has_rule and not (has_diag and not re.match(r"\s*(hold|enter|buy|trade|rebalance)", rule, re.I)):
        kind = "RULE_TEXT_UNMAPPED"
    elif has_diag or hyp:
        kind = "DIAGNOSTIC_QUESTION"
    else:
        kind = "UNRECOGNIZED"

    template: dict[str, Any] | None = None
    if kind == "RULE_STRATEGY":
        m: Match = matches[0]
        spec = TEMPLATES[m.template_id]
        params: dict[str, Any] = {}
        for p in spec.params:
            if p.name in m.explicit:
                params[p.name] = _fv(m.explicit[p.name], FieldClass.EXPLICIT_SOURCE_RULE, m.basis.get(p.name, ""))
            elif m.aliases and p.name in m.aliases:
                params[p.name] = _fv(None, FieldClass.UNDERSPECIFIED, m.aliases[p.name])
            else:
                params[p.name] = _fv(None, FieldClass.UNDERSPECIFIED, "not stated in source")
        template = {"template_id": m.template_id, "params": params,
                    "missing_params": [p.name for p in spec.params if params[p.name]["value"] is None]}
    elif kind == "COMPOSITE_RULE":
        template = {"template_id": None, "components": [m.template_id for m in matches],
                    "missing_params": sorted({f"{m.template_id}.{n}" for m in matches for n in m.missing})}

    unknowns = [name for name, fv in (("direction", dcls), ("asset_class", acls)) if fv in (FieldClass.UNKNOWN, FieldClass.UNDERSPECIFIED)]
    unknowns += ["sizing", "stop_exit_behavior", "transaction_costs", "risk_requirements"]   # no catalog column states these
    if template:
        unknowns += [f"parameter:{n}" for n in template["missing_params"]]
    unmapped = kind in ("COMPOSITE_RULE", "RULE_TEXT_UNMAPPED", "UNRECOGNIZED")
    blockers = blockers_for(asset_class=asset, direction=direction, tags=tags, composite_unmapped=unmapped)
    return {
        "schema": IDEA_SCHEMA,
        "factory_schema": SCHEMA_VERSION,
        "intake_id": intake_id(family, entry["entry_id"]),
        "catalog_family": family,
        "entry_id": entry["entry_id"],
        "entry_kind": entry["entry_kind"],
        "kind": kind,
        "provenance": {"catalog_ledger_sha256": ledger["ledger_sha256"], "catalog_file_sha256": ledger["source"]["sha256"],
                       "profile_id": ledger["profile_id"], "sheet": entry["sheet"], "row_number": entry["row_number"],
                       "entry_content_hash": entry["content_hash"], "entry_canonical_hash": entry["canonical_hash"], "source_refs": f.get("source_refs", []),
                       "source_urls": f.get("source_urls", []), "catalog_labels": entry["labels"],
                       "source_composite_refs": f.get("source_composite_refs", [])},
        "source_text": {"title": title, "hypothesis": hyp, "mechanism": mech, "rule_text": rule, "assets": f.get("assets", ""),
                        "horizon": f.get("horizon", ""), "direction": f.get("direction", ""), "data_needed": f.get("data_needed", "")},
        "economic_hypothesis": _fv(mech or hyp or title, FieldClass.EXPLICIT_SOURCE_RULE if (mech or hyp) else FieldClass.UNDERSPECIFIED,
                                   "source text; a claim, not MQD evidence"),
        "asset_class": _fv(asset, acls, f.get("assets", "")),
        "direction": _fv(direction, dcls, f.get("direction", "")),
        "timeframe": _fv(f.get("horizon", "") or None, FieldClass.UNDERSPECIFIED if f.get("horizon") else FieldClass.UNKNOWN,
                         "holding horizon text, not a bar timeframe"),
        "universe": _fv(None, FieldClass.UNKNOWN, "no catalog universe is authoritative"),
        "entry_exit_rule": _fv(rule or None, (FieldClass.EXPLICIT_SOURCE_RULE if kind == "RULE_STRATEGY" and not template["missing_params"]
                                              else FieldClass.UNDERSPECIFIED if rule else FieldClass.UNKNOWN), rule),
        "sizing": _fv(None, FieldClass.UNKNOWN), "stop_exit_behavior": _fv(None, FieldClass.UNKNOWN),
        "transaction_costs": _fv(None, FieldClass.UNKNOWN), "risk_requirements": _fv(f.get("risk") or None,
                                                                                     FieldClass.UNDERSPECIFIED if f.get("risk") else FieldClass.UNKNOWN, "source caveat"),
        "required_data": tags,
        "template": template,
        "blockers": blockers,
        "known_unknowns": sorted(set(unknowns)),
        "decisions": [],
        "idea_content_hash": sha({"entry": entry["content_hash"], "family": family}),
        "authority": "UNTRUSTED_IDEA_INTAKE",
        "trial_registered": False,
    }


def formalize_ledger(ledger: Mapping[str, Any]) -> list[dict[str, Any]]:
    return [formalize_entry(e, ledger) for e in ledger["entries"]]


def apply_decision(idea: dict[str, Any], decision: Mapping[str, Any]) -> dict[str, Any]:
    """Return a copy of `idea` with ONE operator parameter decision applied. Only a missing parameter of a recognized
    template can be decided (an EXPLICIT source value is never overridden), the value must be inside the template
    domain, and the decision keeps its operator, rationale and reference. The class becomes INFERRED_RULE."""
    required = ("intake_id", "param", "value", "decided_by", "rationale", "decision_ref")
    if any(not decision.get(k) and decision.get(k) != 0 for k in required) or decision["intake_id"] != idea["intake_id"]:
        raise ValueError("a decision needs intake_id, param, value, decided_by, rationale and decision_ref for THIS idea")
    tpl = idea.get("template")
    if not tpl or not tpl.get("template_id"):
        raise ValueError("only a recognized single-template idea can receive a parameter decision")
    name = decision["param"]
    if name not in tpl["params"] or tpl["params"][name]["value"] is not None:
        raise ValueError(f"{name!r} is not a missing parameter of {tpl['template_id']}")
    spec = TEMPLATES[tpl["template_id"]].spec(name)
    v = decision["value"]
    if isinstance(v, bool) or not isinstance(v, int) or not spec.lo <= v <= spec.hi:
        raise ValueError(f"{name}={v!r} outside [{spec.lo}, {spec.hi}]")
    out = _clone(idea)
    t = out["template"]
    t["params"][name] = _fv(v, FieldClass.INFERRED_RULE, f"operator_decision:{decision['decision_ref']}")
    t["missing_params"] = [n for n in t["missing_params"] if n != name]
    out["known_unknowns"] = sorted(set(out["known_unknowns"]) - {f"parameter:{name}"})
    out["decisions"] = [*out["decisions"], {k: decision[k] for k in required}]
    return out


def _clone(obj):
    import copy
    return copy.deepcopy(obj)


FIELD_DOMAINS = {"direction": ("long_flat", "long_only", "long_short", "short_only"),
                 "asset_class": ("equity", "futures", "options", "fx", "crypto")}


def apply_field_decision(idea: dict[str, Any], decision: Mapping[str, Any]) -> dict[str, Any]:
    """Operator decision for an UNKNOWN/UNDERSPECIFIED direction or asset class (for example after reading an AI
    suggestion). A source-EXPLICIT value is never overridden. Blockers are recomputed from the decided value."""
    required = ("intake_id", "field", "value", "decided_by", "rationale", "decision_ref")
    if any(not decision.get(k) for k in required) or decision["intake_id"] != idea["intake_id"]:
        raise ValueError("a field decision needs intake_id, field, value, decided_by, rationale and decision_ref for THIS idea")
    name, value = decision["field"], decision["value"]
    if name not in FIELD_DOMAINS or value not in FIELD_DOMAINS[name]:
        raise ValueError(f"{name}={value!r} is not an allowed decision")
    if idea[name]["class"] == FieldClass.EXPLICIT_SOURCE_RULE.value:
        raise ValueError(f"{name} is stated by the source and cannot be overridden")
    out = _clone(idea)
    out[name] = _fv(value, FieldClass.INFERRED_RULE, f"operator_decision:{decision['decision_ref']}")
    out["blockers"] = blockers_for(asset_class=out["asset_class"]["value"], direction=out["direction"]["value"],
                                   tags=out["required_data"], composite_unmapped=out["kind"] in ("COMPOSITE_RULE", "RULE_TEXT_UNMAPPED", "UNRECOGNIZED"))
    out["known_unknowns"] = sorted(set(out["known_unknowns"]) - {name})
    out["decisions"] = [*out["decisions"], {k: decision[k] for k in required}]
    return out
