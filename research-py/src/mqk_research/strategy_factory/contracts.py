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
    REJECTED_UNDERSPECIFIED = "REJECTED_UNDERSPECIFIED"    # nothing computable can be recovered from the text


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


def sha(obj) -> str:
    return sha256_bytes(canonical_json(obj).encode("utf-8"))


def intake_id(catalog_family: str, entry_id: str) -> str:
    """Stable per (family, entry id): the same idea arriving through a second file copy keeps one identity."""
    return "idea_" + sha({"family": catalog_family, "entry": entry_id})[:24]
