from __future__ import annotations

from typing import Any, Dict, Optional, Tuple

# RESEARCH-LEGACY-TRAINING-BOUNDARY-01
#
# Fail-closed boundary between two structurally different research-artifact
# families:
#   - PROMOTION_GRADE_OOS_EVIDENCE_CLASS: produced only by the registered
#     purged walk-forward path (mqk_research.ml.eval_walkforward,
#     schema_version "walk_forward_eval_v2") or the causal economic
#     walk-forward evaluator built on top of it
#     (mqk_research.ml.economic_walkforward, schema_version
#     "economic_walk_forward_v1"). Both fit standardization/model parameters
#     PER FOLD on train-only rows and report a reserved, unscored holdout
#     (see RESEARCH-PURGED-HOLDOUT-01 / RESEARCH-ECONOMIC-WALKFORWARD-01 --
#     neither evaluator is modified by this module).
#   - DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS: produced by the legacy
#     single-shot path (mqk_research.ml.train / model_logreg, schema_version
#     "ml_train_meta_v1"). fit_logreg_deterministic fits mean/std over
#     whatever full X the caller supplies -- there is no fold loop, so
#     nothing structurally guarantees the rows it was fit on are train-only.
#     This path is legitimate for quick diagnostics but its output must
#     never be mistaken for OOS evidence a promotion decision could rely on.
#
# This module does NOT duplicate walk-forward logic and does NOT redesign
# either evaluator -- it only inspects an already-produced artifact dict and
# decides, structurally, whether it is eligible to be treated as
# promotion-grade OOS evidence. Deliberately does NOT trust a bare
# self-declared "evidence_class" label alone (see train.py's own comment: a
# single string field is trivially flippable) -- eligibility requires BOTH
# the artifact's schema_version to be one of the registered fold-safe
# evaluators' AND the structural shape unique to that schema_version's real
# producer (a non-empty "folds" list, the required top-level keys that
# producer always emits, and a "holdout" block reporting the fixed literal
# {"status": "reserved_not_evaluated", ...}). A single-shot artifact cannot
# satisfy this shape without being rebuilt to mimic the real evaluator's
# entire output -- a materially stronger bar than editing one field.

PROMOTION_GRADE_OOS_EVIDENCE_CLASS = "promotion_grade_oos"
DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS = "diagnostic_or_fit_only"

_RESERVED_HOLDOUT_STATUS = "reserved_not_evaluated"

# schema_version -> required top-level keys that only a genuine, successfully
# completed run of that evaluator populates. Matches the real output shape
# of eval_walkforward.run_walkforward_eval / economic_walkforward's writer,
# read directly off those modules rather than assumed.
_OOS_ELIGIBLE_SCHEMA_VERSIONS: Dict[str, Tuple[str, ...]] = {
    "walk_forward_eval_v2": ("holdout", "folds", "temporal_contract", "spec"),
    "economic_walk_forward_v1": ("holdout", "folds", "protocol", "aggregate"),
}

__all__ = [
    "PROMOTION_GRADE_OOS_EVIDENCE_CLASS",
    "DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS",
    "NotPromotionGradeOosEvidence",
    "classify_research_artifact",
    "require_promotion_grade_oos_evidence",
]


class NotPromotionGradeOosEvidence(RuntimeError):
    """Raised when an artifact fails the promotion-grade OOS evidence boundary."""


def classify_research_artifact(artifact: Dict[str, Any]) -> str:
    """Structural classification, never trusting a self-declared label
    alone. Returns PROMOTION_GRADE_OOS_EVIDENCE_CLASS only if the artifact's
    schema_version is one of the registered fold-safe evaluators' AND it
    carries that evaluator's real structural shape (required top-level keys
    present, non-empty "folds", "holdout" reporting the reserved-not-
    evaluated literal). Everything else -- including an artifact that merely
    CLAIMS a promotion-grade evidence_class without that shape -- is
    DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS."""
    if not isinstance(artifact, dict):
        return DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    schema_version = artifact.get("schema_version")
    required_keys = _OOS_ELIGIBLE_SCHEMA_VERSIONS.get(schema_version)
    if required_keys is None:
        return DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS
    if not all(key in artifact for key in required_keys):
        return DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    folds = artifact.get("folds")
    if not isinstance(folds, list) or len(folds) == 0:
        return DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    holdout = artifact.get("holdout")
    if not isinstance(holdout, dict) or holdout.get("status") != _RESERVED_HOLDOUT_STATUS:
        return DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS

    return PROMOTION_GRADE_OOS_EVIDENCE_CLASS


def require_promotion_grade_oos_evidence(artifact: Dict[str, Any], *, context: Optional[str] = None) -> None:
    """Fail-closed gate for any promotion/dossier consumer that must not
    accidentally treat a diagnostic/single-shot artifact as OOS economic
    evidence. Raises NotPromotionGradeOosEvidence unless
    classify_research_artifact(artifact) == PROMOTION_GRADE_OOS_EVIDENCE_CLASS."""
    actual = classify_research_artifact(artifact)
    if actual != PROMOTION_GRADE_OOS_EVIDENCE_CLASS:
        where = f" ({context})" if context else ""
        raise NotPromotionGradeOosEvidence(
            f"artifact{where} is not promotion-grade OOS evidence (classified as {actual!r}); "
            "refusing to treat it as walk-forward/economic OOS evidence"
        )
