"""Vocabulary shared by the Strategy Factory stages. No behaviour beyond validation and canonical identity helpers."""

from __future__ import annotations

from enum import Enum

from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes


class FieldClass(str, Enum):
    """How a formalized field is known. Missing rules stay missing: nothing is promoted to EXPLICIT by default."""
    EXPLICIT_SOURCE_RULE = "EXPLICIT_SOURCE_RULE"
    INFERRED_RULE = "INFERRED_RULE"
    UNDERSPECIFIED = "UNDERSPECIFIED"
    UNKNOWN = "UNKNOWN"


class Relationship(str, Enum):
    """Roadmap section I2C relationship classes. Result values never decide a relationship."""
    EXACT_DUPLICATE = "EXACT_DUPLICATE"
    PARAMETER_VARIANT = "PARAMETER_VARIANT"
    SEMANTIC_VARIANT = "SEMANTIC_VARIANT"
    COMPLEMENT = "COMPLEMENT"
    MIRROR = "MIRROR"
    COMPOSITE_OF_EXISTING = "COMPOSITE_OF_EXISTING"
    GENUINELY_NEW = "GENUINELY_NEW"
    UNKNOWN_NEEDS_REVIEW = "UNKNOWN_NEEDS_REVIEW"


class Admission(str, Enum):
    """Executable compatibility. A semantic fingerprint alone never makes an idea executable."""
    NATIVE_EXECUTABLE = "NATIVE_EXECUTABLE"
    GRAMMAR_EXPRESSIBLE = "GRAMMAR_EXPRESSIBLE"
    REQUIRES_NEW_IMPLEMENTATION = "REQUIRES_NEW_IMPLEMENTATION"
    REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS = "REQUIRES_UNSUPPORTED_DATA_OR_ECONOMICS"
    REJECTED_INSUFFICIENTLY_SPECIFIED = "REJECTED_INSUFFICIENTLY_SPECIFIED"


class Disposition(str, Enum):
    """The single explicit outcome every submitted catalog entry receives."""
    ADMITTED_NATIVE = "ADMITTED_NATIVE"                    # runs on an existing native engine identity
    ADMITTED_GRAMMAR = "ADMITTED_GRAMMAR"                  # runs on the verified grammar engine
    DUPLICATE_OF_KNOWN = "DUPLICATE_OF_KNOWN"              # exact duplicate of a known or already admitted hypothesis
    NEEDS_FORMALIZATION = "NEEDS_FORMALIZATION"            # a recognized rule with named missing parameters
    NEEDS_OPERATOR_POLICY = "NEEDS_OPERATOR_POLICY"        # economics/universe/direction needs an operator decision
    NEEDS_IMPLEMENTATION = "NEEDS_IMPLEMENTATION"          # recognized, well-specified, no verified executable yet
    UNSUPPORTED_DATA = "UNSUPPORTED_DATA"                  # needs data/economics MQD does not hold or authorize
    DEFERRED_ASSET_CLASS = "DEFERRED_ASSET_CLASS"          # futures/options/FX/crypto: seam preserved, not operational
    DIAGNOSTIC_NOT_STRATEGY = "DIAGNOSTIC_NOT_STRATEGY"    # an event-study / phenomenon question, not a tradable rule
    GOVERNANCE_CONTROL = "GOVERNANCE_CONTROL"              # a control/guardrail row, not an idea
    BENCHMARK_NOT_STRATEGY = "BENCHMARK_NOT_STRATEGY"      # a comparison baseline, never a candidate trial
    REJECTED_UNDERSPECIFIED = "REJECTED_UNDERSPECIFIED"    # nothing computable can be recovered from the text


# The seven coarse dispositions of the reference intake plan (docs/research/knowledge), for reporting only.
COARSE_DISPOSITION = {
    "ADMITTED_NATIVE": "RESEARCH_ELIGIBLE_AWAITING_PREDECLARATION", "ADMITTED_GRAMMAR": "RESEARCH_ELIGIBLE_AWAITING_PREDECLARATION",
    "DUPLICATE_OF_KNOWN": "DUPLICATE_CANDIDATE", "NEEDS_FORMALIZATION": "NEEDS_FORMALIZATION",
    "NEEDS_OPERATOR_POLICY": "NEEDS_FORMALIZATION", "NEEDS_IMPLEMENTATION": "REQUIRES_NEW_NATIVE_IMPLEMENTATION",
    "UNSUPPORTED_DATA": "REQUIRES_UNAVAILABLE_DATA", "DEFERRED_ASSET_CLASS": "REQUIRES_UNAVAILABLE_DATA",
    "DIAGNOSTIC_NOT_STRATEGY": "REFERENCE_ONLY", "GOVERNANCE_CONTROL": "REFERENCE_ONLY", "BENCHMARK_NOT_STRATEGY": "REFERENCE_ONLY",
    "REJECTED_UNDERSPECIFIED": "REJECTED",
}


def proposal_kind(idea_kind: str, blockers: str) -> str:
    """Reference-plan proposal kinds. Controls, benchmarks and diagnostics are never counted as candidate hypotheses."""
    if idea_kind == "GOVERNANCE_CONTROL":
        return "RESEARCH_CONTROL"
    if idea_kind == "BENCHMARK":
        return "BENCHMARK"
    if "F" in blockers:
        return "FUTURE_ASSET"
    if idea_kind == "DIAGNOSTIC_QUESTION":
        return "MECHANISM_DIAGNOSTIC"
    if idea_kind in ("RULE_STRATEGY", "COMPOSITE_RULE"):
        return "STRATEGY_HYPOTHESIS"
    return "INSUFFICIENT_RULES"


# Blocker letters are the vocabulary of experiments/external_idea_intake/disposition.py (kept identical on purpose).
BLOCKERS = {
    "F": "NEEDS_FUTURE_ASSET_SUPPORT",
    "L": "NEEDS_ML_OR_ALT_DATA_FRAMEWORK",
    "D": "NEEDS_ADDITIONAL_AUTHORITATIVE_DATA",
    "P": "NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE",
    "S": "NEEDS_SHORT_OR_BORROW_AUTHORITY",
    "X": "NEEDS_NEW_EXECUTION_POLICY",
    "U": "INSUFFICIENTLY_SPECIFIED",
}
BLOCKER_ORDER = "FLDPSXU"


SYNTHETIC_GRADE = "SYNTHETIC_DIAGNOSTIC"


def promotion_view(evidence_grade: str) -> dict[str, object]:
    """What a Factory output may say about Promotion readiness, from the evidence grade ALONE.

    No Factory evidence grade proves independent out-of-sample readiness: EXPOSED_DEVELOPMENT is declared-exposed
    development data and SYNTHETIC_DIAGNOSTIC is not market evidence. So `promotion_eligible` is always False and the
    readiness is never inferred from "not synthetic". A stored value in an older declaration is never trusted."""
    if evidence_grade == SYNTHETIC_GRADE:
        return {"promotion_eligible": False, "promotion_readiness": "NOT_ELIGIBLE_SYNTHETIC"}
    return {"promotion_eligible": False, "promotion_readiness": "NOT_ESTABLISHED"}


# Scoped to what Factory actions did. It is NOT a reading of the actual MQD Paper/Live runtime or of Promotion state.
FACTORY_AUTHORITY = {
    "scope": "FACTORY_ACTIONS_ONLY: the actual MQD Promotion, Paper and Live runtime state is not read or asserted here",
    "promotion": "NOT_REQUESTED_BY_FACTORY: a Factory campaign creates no Promotion record",
    "paper": "NOT_TOUCHED_BY_FACTORY",
    "live": "NOT_TOUCHED_BY_FACTORY",
}


def sha(obj) -> str:
    return sha256_bytes(canonical_json(obj).encode("utf-8"))


def intake_id(catalog_family: str, entry_id: str) -> str:
    """Stable per (family, entry id): the same idea arriving through a second file copy keeps one identity."""
    return "idea_" + sha({"family": catalog_family, "entry": entry_id})[:24]
