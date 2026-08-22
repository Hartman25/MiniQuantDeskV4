"""
RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01

Canonical Factor Research contract: FactorSpec, FactorObservation,
FactorEvaluationSpec, and FactorEvaluationResult.

Identity discipline (see CLAUDE.md sections 6/19/20):
  - FactorSpec identity is derived ONLY from semantic inputs (family, protocol
    version, params, timing/lookback/horizon semantics, universe identity,
    data provenance identity). Result values (IC, Sharpe, p-values, pass/fail)
    are NEVER part of factor identity.
  - FactorEvaluationSpec identity is derived ONLY from the evaluation
    protocol/window/universe binding, never from the evaluation's own result.
  - Transport/layout-only fields (e.g. `layout_note`) are explicitly excluded
    from identity payloads so relocating output files never mints a new
    factor or evaluation identity.

This module is pure data + validation. No I/O, no DB, no CLI logic.
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Dict, List, Optional

from mqk_research.exp_distributed.hashing import short_hash

FACTOR_CONTRACT_SCHEMA_VERSION = "factor_contract_v1"
FACTOR_EVALUATION_CONTRACT_SCHEMA_VERSION = "factor_evaluation_contract_v1"

DIRECTION_HIGHER_IS_BETTER = "higher_is_better"
DIRECTION_LOWER_IS_BETTER = "lower_is_better"
_VALID_DIRECTIONS = (DIRECTION_HIGHER_IS_BETTER, DIRECTION_LOWER_IS_BETTER)

NORMALIZATION_RAW = "raw"
NORMALIZATION_CROSS_SECTIONAL_RANK = "cross_sectional_rank"
NORMALIZATION_CROSS_SECTIONAL_ZSCORE = "cross_sectional_zscore"
_VALID_NORMALIZATIONS = (
    NORMALIZATION_RAW,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    NORMALIZATION_CROSS_SECTIONAL_ZSCORE,
)

# Distinguishes a value known only after its own bar's close (same-bar-close)
# from a value whose tradable/evaluation timing is deferred to the following
# bar (next-bar-tradable). Never left implicit — a classification LABEL must
# never imply executable same-bar timing.
TIMING_SAME_BAR_CLOSE = "same_bar_close_known_after_close"
TIMING_NEXT_BAR_TRADABLE = "next_bar_tradable"
_VALID_TIMING_CONVENTIONS = (TIMING_SAME_BAR_CLOSE, TIMING_NEXT_BAR_TRADABLE)

EVAL_STATUS_SUCCEEDED = "succeeded"
EVAL_STATUS_FAILED = "failed"
EVAL_STATUS_NOT_EVALUABLE = "not_evaluable"
_VALID_EVAL_STATUSES = (EVAL_STATUS_SUCCEEDED, EVAL_STATUS_FAILED, EVAL_STATUS_NOT_EVALUABLE)


def _parse_utc(ts: str, *, field_name: str) -> datetime:
    if not isinstance(ts, str) or not ts.strip():
        raise ValueError(f"{field_name} must be a non-empty ISO-8601 UTC timestamp string")
    normalized = ts.strip().replace("Z", "+00:00")
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as exc:
        raise ValueError(f"{field_name} is not a valid ISO-8601 timestamp: {ts!r}") from exc
    if parsed.tzinfo is None:
        raise ValueError(f"{field_name} must carry an explicit UTC offset/Z suffix: {ts!r}")
    return parsed


@dataclass(frozen=True)
class FactorSpec:
    """Identity-bearing specification of a factor candidate.

    Every field here contributes to `factor_id` EXCEPT `layout_note`, which
    is explicitly transport/display-only (see CLAUDE.md: "Transport/layout-
    only changes MUST NOT create new research candidates").
    """

    family: str
    name: str
    protocol_version: str
    params: Dict[str, Any] = field(default_factory=dict)
    required_input_fields: List[str] = field(default_factory=list)
    lookback_periods: int = 0
    horizon_periods: int = 1
    normalization: str = NORMALIZATION_RAW
    direction: str = DIRECTION_HIGHER_IS_BETTER
    universe_identity: Dict[str, Any] = field(default_factory=dict)
    data_provenance_identity: Dict[str, Any] = field(default_factory=dict)
    timing_convention: str = TIMING_NEXT_BAR_TRADABLE
    information_lag_periods: int = 0
    # Transport/layout-only. Never part of identity_payload().
    layout_note: str = ""

    def validate(self) -> None:
        if not self.family.strip():
            raise ValueError("FactorSpec.family is required")
        if not self.name.strip():
            raise ValueError("FactorSpec.name is required")
        if not self.protocol_version.strip():
            raise ValueError("FactorSpec.protocol_version is required")
        if not self.required_input_fields:
            raise ValueError("FactorSpec.required_input_fields must be non-empty")
        if any(not str(f).strip() for f in self.required_input_fields):
            raise ValueError("FactorSpec.required_input_fields cannot contain empty entries")
        if self.lookback_periods < 0:
            raise ValueError("FactorSpec.lookback_periods must be >= 0")
        if self.horizon_periods < 1:
            raise ValueError("FactorSpec.horizon_periods must be >= 1")
        if self.information_lag_periods < 0:
            raise ValueError("FactorSpec.information_lag_periods must be >= 0")
        if self.direction not in _VALID_DIRECTIONS:
            raise ValueError(f"FactorSpec.direction must be one of {_VALID_DIRECTIONS}: got {self.direction!r}")
        if self.normalization not in _VALID_NORMALIZATIONS:
            raise ValueError(
                f"FactorSpec.normalization must be one of {_VALID_NORMALIZATIONS}: got {self.normalization!r}"
            )
        if self.timing_convention not in _VALID_TIMING_CONVENTIONS:
            raise ValueError(
                f"FactorSpec.timing_convention must be one of {_VALID_TIMING_CONVENTIONS}: "
                f"got {self.timing_convention!r}"
            )
        if not isinstance(self.universe_identity, dict):
            raise ValueError("FactorSpec.universe_identity must be a dict")
        if not isinstance(self.data_provenance_identity, dict):
            raise ValueError("FactorSpec.data_provenance_identity must be a dict")
        if not isinstance(self.params, dict):
            raise ValueError("FactorSpec.params must be a dict")

    def identity_payload(self) -> Dict[str, Any]:
        """Canonical, result-independent identity payload. Deliberately
        excludes `layout_note` — see class docstring.

        Deep-copies mutable nested fields (params/universe_identity/
        data_provenance_identity) so a caller mutating the returned dict can
        never retroactively change this spec's own identity."""
        return {
            "schema_version": FACTOR_CONTRACT_SCHEMA_VERSION,
            "family": self.family,
            "name": self.name,
            "protocol_version": self.protocol_version,
            "params": copy.deepcopy(self.params),
            "required_input_fields": sorted(str(f) for f in self.required_input_fields),
            "lookback_periods": self.lookback_periods,
            "horizon_periods": self.horizon_periods,
            "normalization": self.normalization,
            "direction": self.direction,
            "universe_identity": copy.deepcopy(self.universe_identity),
            "data_provenance_identity": copy.deepcopy(self.data_provenance_identity),
            "timing_convention": self.timing_convention,
            "information_lag_periods": self.information_lag_periods,
        }

    def compute_factor_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)


@dataclass(frozen=True)
class FactorObservation:
    """One canonical factor observation.

    Hard invariant: information_cutoff_ts_utc <= observation_ts_utc. A factor
    may consume no source observation published after its information
    cutoff — this dataclass only proves the observation/cutoff pair is not
    self-contradictory; it does not itself audit upstream inputs.
    """

    factor_id: str
    symbol: str
    observation_ts_utc: str
    information_cutoff_ts_utc: str
    value: float
    input_provenance_identity: Dict[str, Any] = field(default_factory=dict)

    def validate(self) -> None:
        if not self.factor_id.strip():
            raise ValueError("FactorObservation.factor_id is required")
        if not self.symbol.strip():
            raise ValueError("FactorObservation.symbol is required")
        observation_ts = _parse_utc(self.observation_ts_utc, field_name="observation_ts_utc")
        cutoff_ts = _parse_utc(self.information_cutoff_ts_utc, field_name="information_cutoff_ts_utc")
        if cutoff_ts > observation_ts:
            raise ValueError(
                "FactorObservation violates causality: information_cutoff_ts_utc "
                f"({self.information_cutoff_ts_utc}) is after observation_ts_utc "
                f"({self.observation_ts_utc})"
            )
        try:
            value = float(self.value)
        except (TypeError, ValueError) as exc:
            raise ValueError("FactorObservation.value must be a finite number") from exc
        if value != value or value in (float("inf"), float("-inf")):
            raise ValueError("FactorObservation.value must be finite (no NaN/Inf)")
        if not isinstance(self.input_provenance_identity, dict):
            raise ValueError("FactorObservation.input_provenance_identity must be a dict")


@dataclass(frozen=True)
class FactorEvaluationSpec:
    """Identity-bearing binding of a factor to an evaluation window/universe.

    Excludes any metric/result value by construction — `FactorEvaluationResult`
    is a separate type and never feeds back into this identity.
    """

    factor_id: str
    universe_identity: Dict[str, Any]
    evaluation_window_start_utc: str
    evaluation_window_end_utc: str
    label_protocol_version: str
    evaluation_protocol_version: str

    def validate(self) -> None:
        if not self.factor_id.strip():
            raise ValueError("FactorEvaluationSpec.factor_id is required")
        if not self.label_protocol_version.strip():
            raise ValueError("FactorEvaluationSpec.label_protocol_version is required")
        if not self.evaluation_protocol_version.strip():
            raise ValueError("FactorEvaluationSpec.evaluation_protocol_version is required")
        if not isinstance(self.universe_identity, dict):
            raise ValueError("FactorEvaluationSpec.universe_identity must be a dict")
        start_ts = _parse_utc(self.evaluation_window_start_utc, field_name="evaluation_window_start_utc")
        end_ts = _parse_utc(self.evaluation_window_end_utc, field_name="evaluation_window_end_utc")
        if start_ts >= end_ts:
            raise ValueError("FactorEvaluationSpec.evaluation_window_start_utc must be before ...end_utc")

    def identity_payload(self) -> Dict[str, Any]:
        return {
            "schema_version": FACTOR_EVALUATION_CONTRACT_SCHEMA_VERSION,
            "factor_id": self.factor_id,
            "universe_identity": copy.deepcopy(self.universe_identity),
            "evaluation_window_start_utc": self.evaluation_window_start_utc,
            "evaluation_window_end_utc": self.evaluation_window_end_utc,
            "label_protocol_version": self.label_protocol_version,
            "evaluation_protocol_version": self.evaluation_protocol_version,
        }

    def compute_evaluation_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)


@dataclass(frozen=True)
class FactorEvaluationResult:
    """Outcome of one factor evaluation attempt.

    `reason` is required whenever `status != "succeeded"` — a failed or
    not_evaluable outcome must never be recorded silently. Result values
    here (metrics) never alter `factor_id` or `evaluation_id`.
    """

    eval_id: str
    factor_id: str
    status: str
    metrics: Dict[str, Any] = field(default_factory=dict)
    reason: Optional[str] = None
    artifact_paths: Dict[str, str] = field(default_factory=dict)

    def validate(self) -> None:
        if not self.eval_id.strip():
            raise ValueError("FactorEvaluationResult.eval_id is required")
        if not self.factor_id.strip():
            raise ValueError("FactorEvaluationResult.factor_id is required")
        if self.status not in _VALID_EVAL_STATUSES:
            raise ValueError(
                f"FactorEvaluationResult.status must be one of {_VALID_EVAL_STATUSES}: got {self.status!r}"
            )
        if self.status != EVAL_STATUS_SUCCEEDED and not (self.reason and self.reason.strip()):
            raise ValueError("FactorEvaluationResult.reason is required when status is not 'succeeded'")
        if not isinstance(self.metrics, dict):
            raise ValueError("FactorEvaluationResult.metrics must be a dict")
