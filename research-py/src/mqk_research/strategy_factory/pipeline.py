"""Intake pipeline: catalog ledgers -> formalized ideas -> relationships -> explicit dispositions.

Pure and deterministic for a given (ledgers, known index, operator decisions). Every submitted entry appears in the
output exactly once; the disposition counts always sum to the submitted-entry count (checked, not assumed).
"""

from __future__ import annotations

from collections import Counter
from typing import Any, Iterable, Mapping, Sequence

from mqk_research.strategy_factory.admission import decide
from mqk_research.strategy_factory.contracts import sha
from mqk_research.strategy_factory.dedup import apply_novelty_review, dedup_population
from mqk_research.strategy_factory.formalize import apply_decision, apply_field_decision, formalize_ledger
from mqk_research.strategy_factory.known_index import KnownEntry


def process(ledgers: Sequence[Mapping[str, Any]], known: Sequence[KnownEntry], *, grammar_available: bool,
            decisions: Iterable[Mapping[str, Any]] = (), reviews: Iterable[Mapping[str, Any]] = ()) -> dict[str, Any]:
    ideas: dict[str, dict[str, Any]] = {}
    submitted = 0
    copies: list[dict[str, str]] = []
    for ledger in sorted(ledgers, key=lambda l: (l["catalog_family"], l["source"]["sha256"])):
        for idea in formalize_ledger(ledger):
            submitted += 1
            prior = ideas.get(idea["intake_id"])
            if prior is None:
                ideas[idea["intake_id"]] = idea
            elif prior["provenance"]["entry_canonical_hash"] == idea["provenance"]["entry_canonical_hash"]:
                copies.append({"intake_id": idea["intake_id"], "copy_of_file": prior["provenance"]["catalog_file_sha256"],
                               "this_file": idea["provenance"]["catalog_file_sha256"], "disposition": "DUPLICATE_SOURCE_COPY"})
            else:
                raise ValueError(f"source conflict for {idea['intake_id']}: same entry id, different content across files")
    for d in sorted(decisions, key=lambda d: (d["intake_id"], d.get("field") or "", d.get("param") or "", d["decision_ref"])):
        if d["intake_id"] not in ideas:
            raise ValueError(f"decision names an unknown idea {d['intake_id']}")
        ideas[d["intake_id"]] = (apply_field_decision if "field" in d else apply_decision)(ideas[d["intake_id"]], d)
    rel = dedup_population(list(ideas.values()), known)
    for r in sorted(reviews, key=lambda r: r["intake_id"]):
        rel[r["intake_id"]] = apply_novelty_review(rel[r["intake_id"]], r)
    records = []
    for iid in sorted(ideas):
        idea = ideas[iid]
        verdict = decide(idea, rel[iid], grammar_available=grammar_available)
        records.append({**idea, "dedup": rel[iid], **{k: verdict[k] for k in ("admission", "disposition", "execution_path", "reasons")}})
    counts = Counter(r["disposition"] for r in records)
    if sum(counts.values()) != len(records) or len(records) + len(copies) != submitted:
        raise AssertionError("disposition accounting does not cover the submitted population")
    return {
        "schema": "strategy_factory_intake_result_v1",
        "submitted_entries": submitted,
        "unique_ideas": len(records),
        "source_copies": copies,
        "disposition_counts": dict(sorted(counts.items())),
        "relationship_counts": dict(sorted(Counter(r["dedup"]["relationship"] for r in records).items())),
        "admission_counts": dict(sorted(Counter(r["admission"] for r in records).items())),
        "records": records,
        "result_sha256": sha([{k: r[k] for k in ("intake_id", "disposition", "admission", "execution_path")} for r in records]),
    }
