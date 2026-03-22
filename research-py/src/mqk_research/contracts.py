from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional


# Stable interface contract version for Research output artifacts.
# This is NOT the same as the "schema_version" field in manifest.json (kept for backward compatibility).
CONTRACT_VERSION: str = "1"


def _json_dumps_deterministic(obj: Any, *, indent: int = 2) -> str:
    # Deterministic JSON: stable keys + newline handled by caller.
    return json.dumps(obj, indent=indent, sort_keys=True, ensure_ascii=False)


@dataclass(frozen=True)
class ResearchUniverseRecord:
    """
    Stable typed record for universe.csv rows.
    Keep required fields minimal and stable; allow extensions via extras.
    """
    instrument_id: str
    symbol: str
    asset_class: str

    # Common optional fields (present in Phase 1 universe.csv today)
    rank: Optional[int] = None
    included: Optional[bool] = None

    # Forward-compatible extension bag (pure data; caller controls content)
    extras: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class ResearchTargetRecord:
    """
    Stable typed record for targets.csv rows.
    """
    instrument_id: str
    symbol: str
    asset_class: str
    side: str
    weight: float

    extras: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class ResearchIntent:
    """
    Stable typed schema for intent.json (non-equity placeholder artifact).
    """
    schema_version: str
    contract_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    asset_class: str
    symbols: List[str]
    pipeline: str
    notes: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


@dataclass(frozen=True)
class ResearchManifest:
    """
    Stable typed schema for manifest.json.
    Pure data only — no I/O, no DB, no CLI logic.
    """
    schema_version: str
    contract_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    policy_path: str
    policy_sha256: str
    params: Dict[str, Any]
    inputs: Dict[str, Any]
    outputs: Dict[str, Any]
    notes: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


def validate_contract_version(version: str) -> None:
    if version != CONTRACT_VERSION:
        raise ValueError(
            f"Research contract version mismatch: expected={CONTRACT_VERSION} got={version}. "
            "Refusing to write artifacts."
        )


# ---------------------------------------------------------------------------
# TV-01: Promoted Artifact Contract
#
# A promoted artifact has ONE stable identity, ONE canonical manifest, and
# ONE explicit layout rule.  Producer and consumer share this contract.
# ---------------------------------------------------------------------------

# Schema version for the promoted artifact manifest.
# Bump to "promoted-v2" only with an explicit migration path — never silently
# change field semantics under an existing version string.
PROMOTED_ARTIFACT_CONTRACT_VERSION: str = "promoted-v1"

# Required files for a signal_pack promoted artifact.
# Any consumer must verify these exist before treating the artifact as valid.
SIGNAL_PACK_REQUIRED_FILES: List[str] = [
    "signals.csv",
    "signal_pack.json",
    "promoted_manifest.json",
]


@dataclass(frozen=True)
class PromotedArtifactLineage:
    """
    Lineage captured at promotion time.

    Links a promoted artifact back to its research-side origin so any consumer
    can trace what produced it.  All IDs here are content-addressed — they are
    derived from file content, not from wall-clock time or random values.
    """
    # Content-addressed ID of signal_pack.json (sha256_json of the full dict).
    # This is also the artifact_id and the directory name.
    signal_pack_id: str
    # Schema version string from signal_pack.json (e.g. "signal_pack_v1").
    signal_pack_schema_version: str
    # dataset_id from signal_pack.json ids block, if present.
    dataset_id: Optional[str]
    # model_id from signal_pack.json ids block, if present.
    model_id: Optional[str]
    # Path from project_root to the research run dir that was promoted.
    # Relative (posix) where possible; absolute fallback if run_dir is outside project_root.
    source_dir: str


@dataclass(frozen=True)
class PromotedArtifactManifest:
    """
    Canonical manifest written to promoted/signal_packs/<artifact_id>/promoted_manifest.json.

    This is the single authoritative contract that any consumer reads to understand:
    - what artifact this is (artifact_id, artifact_type, schema_version)
    - what stage produced it (stage, produced_by)
    - where its files are (data_root, required_files, optional_files)
    - where it came from (lineage)

    Layout rule: given artifact_id, data lives at:
        <project_root>/<data_root>/  ==  promoted/signal_packs/<artifact_id>/

    The data_root field makes this rule explicit and portable — consumers never
    need to reconstruct the path from scratch.

    schema_version identifies the contract format.  Never change field semantics
    without bumping to "promoted-v2" and writing an explicit migration path.

    artifact_id is content-addressed (= sha256_json(signal_pack.json)) and
    deterministic.  It is both the directory name and the canonical ID.
    """
    schema_version: str        # always PROMOTED_ARTIFACT_CONTRACT_VERSION
    artifact_id: str           # content-addressed, deterministic; also the directory name
    artifact_type: str         # "signal_pack"
    stage: str                 # always "promoted"
    produced_by: str           # always "research-py"
    data_root: str             # posix-relative: promoted/signal_packs/<artifact_id>
    required_files: List[str]  # files that MUST exist for a valid artifact
    optional_files: List[str]  # files that MAY exist (empty until future additions)
    lineage: PromotedArtifactLineage
    produced_at_utc: str       # ISO-8601 UTC; informational only, not a key field

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


def derive_artifact_id(artifact_type: str, source_id: str) -> str:
    """
    Derive the canonical artifact_id from artifact_type and source content hash.

    For signal_pack artifacts:
        source_id = sha256_json(signal_pack_dict)  (the content-addressed ID of signal_pack.json)
        artifact_id = derive_artifact_id("signal_pack", source_id) = source_id

    The function is an identity for single-type IDs.  It exists to make the
    derivation rule explicit rather than relying on the implicit convention that
    "we just use sha256_json(sp)".  If the scheme ever changes (e.g. type-namespaced
    IDs), update this function; all callers automatically update.

    Returns: the full 64-char hex SHA256 of source_id (= source_id itself for
    signal_pack, since source_id is already the authoritative content hash).
    """
    # Signal_pack artifact_id is the content hash of signal_pack.json.
    # Encoding the type here ensures future multi-type namespacing works correctly
    # while keeping backward compatibility: sha256({"artifact_type": "signal_pack",
    # "source_id": <sp_sha256>}) is stable and distinct from any other type.
    payload = json.dumps(
        {"artifact_type": artifact_type, "source_id": source_id},
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def read_promoted_manifest(manifest_path: Path) -> PromotedArtifactManifest:
    """
    Consumer reader: load and validate a promoted_manifest.json.

    This is the canonical consumer entry point.  Any downstream stage
    (backtest, execution-facing tooling, shadow pipeline) should use this
    function rather than reading the JSON directly.

    The caller locates the manifest at:
        <project_root>/<data_root>/promoted_manifest.json
    or equivalently:
        promoted/signal_packs/<artifact_id>/promoted_manifest.json

    Raises:
        FileNotFoundError: manifest_path does not exist.
        ValueError: schema_version or stage field does not match expected values.
    Returns:
        PromotedArtifactManifest with all fields populated and validated.
    """
    manifest_path = Path(manifest_path)
    if not manifest_path.exists():
        raise FileNotFoundError(f"promoted_manifest.json not found: {manifest_path}")

    raw = json.loads(manifest_path.read_text(encoding="utf-8"))

    sv = raw.get("schema_version")
    if sv != PROMOTED_ARTIFACT_CONTRACT_VERSION:
        raise ValueError(
            f"promoted_manifest schema_version mismatch: "
            f"expected={PROMOTED_ARTIFACT_CONTRACT_VERSION!r} got={sv!r}"
        )

    stage = raw.get("stage")
    if stage != "promoted":
        raise ValueError(f"promoted_manifest stage must be 'promoted', got {stage!r}")

    lin = raw.get("lineage", {})
    lineage = PromotedArtifactLineage(
        signal_pack_id=lin["signal_pack_id"],
        signal_pack_schema_version=lin["signal_pack_schema_version"],
        dataset_id=lin.get("dataset_id"),
        model_id=lin.get("model_id"),
        source_dir=lin["source_dir"],
    )

    return PromotedArtifactManifest(
        schema_version=raw["schema_version"],
        artifact_id=raw["artifact_id"],
        artifact_type=raw["artifact_type"],
        stage=raw["stage"],
        produced_by=raw["produced_by"],
        data_root=raw["data_root"],
        required_files=raw["required_files"],
        optional_files=raw.get("optional_files", []),
        lineage=lineage,
        produced_at_utc=raw["produced_at_utc"],
    )


# ---------------------------------------------------------------------------
# TV-02: Deployability Gate Contract
#
# A candidate artifact can be evaluated deterministically for minimum
# tradability and sample adequacy.  The gate result is explicit,
# machine-readable, and keyed to the canonical artifact_id from TV-01.
#
# Passing this gate does NOT prove edge, profitability, or live trust.
# It only confirms minimum viable tradability and sample adequacy criteria.
# ---------------------------------------------------------------------------

# Schema version for the deployability gate result.
# Bump to "gate-v2" only with an explicit migration path.
DEPLOYABILITY_GATE_CONTRACT_VERSION: str = "gate-v1"


@dataclass(frozen=True)
class DeployabilityCheck:
    """
    One explicit, named check in the deployability gate.

    Every check exposes the observed value, the threshold applied, and the
    pass/fail result so any consumer can audit the decision without re-running
    the evaluator.
    """
    name: str        # stable check identifier (e.g. "min_trade_count")
    passed: bool     # True if the check passed
    value: float     # observed metric value
    threshold: float # threshold applied to determine pass/fail
    note: str        # why this check matters; human-readable


@dataclass(frozen=True)
class DeployabilityGateResult:
    """
    Output of the deployability gate for a candidate artifact.

    Keyed to the canonical artifact_id from TV-01 so any downstream consumer
    can unambiguously associate this gate result with its artifact.

    All checks are explicit and deterministic — the same input metrics and
    the same config always produce the same result.

    passed=True means this artifact meets minimum tradability and sample
    adequacy criteria for downstream consideration.
    passed=True is NOT proof of edge, profitability, or live trust.
    passed=False means at least one explicit minimum criterion failed.
    """
    schema_version: str                   # always DEPLOYABILITY_GATE_CONTRACT_VERSION
    artifact_id: str                      # canonical artifact ID (TV-01)
    passed: bool                          # True iff all checks pass
    checks: List[DeployabilityCheck]      # per-check results; all checks always present
    overall_reason: str                   # summary of why passed or failed
    evaluated_at_utc: str                 # ISO-8601 UTC; informational only

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


def read_deployability_gate(gate_path: Path) -> DeployabilityGateResult:
    """
    Consumer reader: load and validate a deployability_gate.json.

    Raises:
        FileNotFoundError: gate_path does not exist.
        ValueError: schema_version does not match DEPLOYABILITY_GATE_CONTRACT_VERSION.
    Returns:
        DeployabilityGateResult with all checks populated.
    """
    gate_path = Path(gate_path)
    if not gate_path.exists():
        raise FileNotFoundError(f"deployability_gate.json not found: {gate_path}")

    raw = json.loads(gate_path.read_text(encoding="utf-8"))

    sv = raw.get("schema_version")
    if sv != DEPLOYABILITY_GATE_CONTRACT_VERSION:
        raise ValueError(
            f"deployability_gate schema_version mismatch: "
            f"expected={DEPLOYABILITY_GATE_CONTRACT_VERSION!r} got={sv!r}"
        )

    checks = [
        DeployabilityCheck(
            name=c["name"],
            passed=bool(c["passed"]),
            value=float(c["value"]),
            threshold=float(c["threshold"]),
            note=c["note"],
        )
        for c in raw.get("checks", [])
    ]

    return DeployabilityGateResult(
        schema_version=raw["schema_version"],
        artifact_id=raw["artifact_id"],
        passed=bool(raw["passed"]),
        checks=checks,
        overall_reason=raw["overall_reason"],
        evaluated_at_utc=raw["evaluated_at_utc"],
    )


# ---------------------------------------------------------------------------
# TV-03: Parity Evidence Contract
#
# The parity evidence manifest chains the full audit trail:
#   artifact_id (TV-01) → gate result (TV-02) → shadow evidence → comparison basis
#
# This artifact records what evidence exists for a candidate's shadow/live
# parity.  It does NOT claim live trust.
#
# live_trust_complete=False is the required state from this patch.
# It can only become True when LO-03 operator proof is complete.
# ---------------------------------------------------------------------------

# Schema version for the parity evidence manifest.
# Bump to "parity-v2" only with an explicit migration path.
PARITY_EVIDENCE_CONTRACT_VERSION: str = "parity-v1"


@dataclass(frozen=True)
class ShadowEvidenceRef:
    """
    Reference to shadow evaluation evidence for a candidate artifact.

    Uses an explicit evidence_available flag rather than fabricating metrics.
    All score fields are Optional — None means "not computed", never zero.

    When evidence_available=False, all score fields will be None.
    When evidence_available=True, score fields may still be None if the
    specific metric was not computed in the shadow evaluation run.
    """
    shadow_label_run_id: Optional[str]   # ID from shadow_label_meta.json (ids.label_run_id)
    labeled_rows: Optional[int]          # row count of labeled targets.csv
    precision: Optional[float]           # classification precision, if computed
    recall: Optional[float]              # classification recall, if computed
    f1: Optional[float]                  # F1 score, if computed
    evidence_available: bool             # True iff shadow evaluation was actually run
    evidence_note: str                   # brief description of what evidence exists/is missing


@dataclass(frozen=True)
class ParityEvidenceManifest:
    """
    Canonical parity evidence manifest.

    Written to promoted/signal_packs/<artifact_id>/parity_evidence.json.

    Chain of custody:
        artifact_id       (TV-01 — canonical artifact identity)
        → gate_passed     (TV-02 — deployability gate result)
        → shadow_evidence (TV-03 — shadow evaluation reference)
        → comparison_basis (what live-facing assessment this is against)

    HONESTY CONSTRAINT:
        live_trust_complete=False is the only valid state written by this patch.
        Setting it True requires LO-03 operator proof and is not permitted here.

        This manifest records what evidence exists and makes trust gaps explicit.
        It does NOT certify the strategy for live capital deployment.
    """
    schema_version: str                 # always PARITY_EVIDENCE_CONTRACT_VERSION
    artifact_id: str                    # canonical artifact ID (TV-01)
    gate_passed: bool                   # TV-02 result; False = did not clear deployability gate
    gate_schema_version: str            # schema_version from the TV-02 gate result (audit linkage)
    shadow_evidence: ShadowEvidenceRef  # TV-03 shadow evidence reference
    comparison_basis: str               # live-facing comparison description (explicit, not vague)
    live_trust_complete: bool           # ALWAYS False; set only by LO-03 operator proof
    live_trust_gaps: List[str]          # explicit remaining gaps before live trust is complete
    produced_at_utc: str                # ISO-8601 UTC; informational only

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


def read_parity_evidence(evidence_path: Path) -> ParityEvidenceManifest:
    """
    Consumer reader: load and validate a parity_evidence.json.

    Raises:
        FileNotFoundError: evidence_path does not exist.
        ValueError: schema_version does not match PARITY_EVIDENCE_CONTRACT_VERSION.
    Returns:
        ParityEvidenceManifest with all fields populated.
    """
    evidence_path = Path(evidence_path)
    if not evidence_path.exists():
        raise FileNotFoundError(f"parity_evidence.json not found: {evidence_path}")

    raw = json.loads(evidence_path.read_text(encoding="utf-8"))

    sv = raw.get("schema_version")
    if sv != PARITY_EVIDENCE_CONTRACT_VERSION:
        raise ValueError(
            f"parity_evidence schema_version mismatch: "
            f"expected={PARITY_EVIDENCE_CONTRACT_VERSION!r} got={sv!r}"
        )

    se = raw.get("shadow_evidence", {})
    shadow_evidence = ShadowEvidenceRef(
        shadow_label_run_id=se.get("shadow_label_run_id"),
        labeled_rows=se.get("labeled_rows"),
        precision=se.get("precision"),
        recall=se.get("recall"),
        f1=se.get("f1"),
        evidence_available=bool(se["evidence_available"]),
        evidence_note=se["evidence_note"],
    )

    return ParityEvidenceManifest(
        schema_version=raw["schema_version"],
        artifact_id=raw["artifact_id"],
        gate_passed=bool(raw["gate_passed"]),
        gate_schema_version=raw["gate_schema_version"],
        shadow_evidence=shadow_evidence,
        comparison_basis=raw["comparison_basis"],
        live_trust_complete=bool(raw["live_trust_complete"]),
        live_trust_gaps=list(raw["live_trust_gaps"]),
        produced_at_utc=raw["produced_at_utc"],
    )


# ---------------------------------------------------------------------------
# TV-04: Portfolio allocation contracts
#
# Proves that multiple strategies cannot all assume ideal capital simultaneously.
# Allocation is explicit, deterministic, and bounded by a capital budget.
#
# Schema version: allocation-v2
#   Changed from allocation-v1:
#   - StrategyAllocation gains adjustment_reasons: List[str] (multi-cause truth).
#     rejection_reason is now reserved for status=="rejected" only.
#   - PortfolioAllocationManifest gains unallocated_capital_micros: int (exact
#     capital accounting: sum(allocated) + unallocated == total exactly).
# ---------------------------------------------------------------------------

PORTFOLIO_ALLOCATION_CONTRACT_VERSION: str = "allocation-v2"


@dataclass
class CapitalBudget:
    """Hard limits on how capital is distributed across the strategy fleet."""

    total_capital_micros: int
    """Total deployable capital in micros (1 USD = 1_000_000 micros)."""

    max_strategies: int
    """Maximum number of strategies that may receive a non-zero allocation."""

    max_single_strategy_fraction: float
    """No single strategy may receive more than this fraction (0 < x <= 1.0)."""


@dataclass
class StrategyCandidate:
    """A strategy that is requesting a capital allocation.

    Consumed from TV-01 (artifact_id / signal_pack_id) and TV-02 (gate_passed).
    """

    artifact_id: str
    """Stable artifact identity from TV-01 PromotedArtifactManifest."""

    signal_pack_id: str
    """Signal-pack identity from TV-01 PromotedArtifactLineage."""

    gate_passed: bool
    """Whether TV-02 deployability gate was satisfied for this artifact."""

    requested_fraction: float
    """Fraction of total capital this strategy requests (0 < x <= 1.0)."""


@dataclass
class StrategyAllocation:
    """The outcome of the allocation process for one strategy candidate."""

    artifact_id: str

    status: str
    """One of: 'allocated', 'budget_capped', 'rejected'."""

    allocated_fraction: float
    """Fraction of total capital actually allocated. 0.0 if rejected.
    Derived from actual assigned micros:
      allocated_fraction == allocated_capital_micros / budget.total_capital_micros
    Never derived from a pre-truncation float; always consistent with micros."""

    allocated_capital_micros: int
    """Absolute capital in micros. 0 if rejected.  Source of truth for this
    allocation row; allocated_fraction is derived from this value."""

    rejection_reason: Optional[str]
    """Set only when status == 'rejected'.  None for 'allocated' and 'budget_capped'.
    Values: 'gate_failed', 'invalid_requested_fraction', 'max_strategies_reached'."""

    adjustment_reasons: List[str] = field(default_factory=list)
    """Set when status == 'budget_capped'; empty for 'allocated' and 'rejected'.
    Contains one or both of:
      'single_strategy_cap'  — capped by max_single_strategy_fraction.
      'budget_contention'    — scaled down because total demand exceeded 1.0.
    Both causes may appear simultaneously when the per-strategy cap fires AND the
    scaled total still exceeds 1.0."""


@dataclass
class PortfolioAllocationManifest:
    """Complete output of a portfolio allocation run.

    schema_version == PORTFOLIO_ALLOCATION_CONTRACT_VERSION.

    Invariants:
    - total_allocated_fraction <= 1.0 always.
    - sum(a.allocated_capital_micros for non-rejected a)
      + unallocated_capital_micros == budget.total_capital_micros exactly.
    - sum(a.allocated_capital_micros for non-rejected a) <= budget.total_capital_micros.
    - allocations are sorted by artifact_id (canonical output order).
    """

    schema_version: str
    budget: CapitalBudget
    allocations: List[StrategyAllocation]
    total_allocated_fraction: float
    """Derived from actual assigned micros:
      total_allocated_fraction == sum(non-rejected allocated_capital_micros)
                                   / budget.total_capital_micros
    Never derived from summing pre-truncation float fractions."""
    allocated_count: int
    rejected_count: int
    unallocated_capital_micros: int
    """Exact integer micros not assigned to any strategy.
    Covers both by-design undeployed capital and integer-truncation shortfall.
    Always >= 0.  Zero only when every micro is exactly allocated.
    Invariant: sum(allocated_capital_micros) + unallocated_capital_micros
               == budget.total_capital_micros exactly."""
    selection_method: str
    """Always 'gate_then_rank_by_requested_fraction'."""
    produced_at_utc: str


def read_portfolio_allocation(path: Path) -> "PortfolioAllocationManifest":
    """Load and validate a PortfolioAllocationManifest from *path*."""
    import json

    raw = json.loads(path.read_text(encoding="utf-8"))
    sv = raw.get("schema_version", "")
    if sv != PORTFOLIO_ALLOCATION_CONTRACT_VERSION:
        raise ValueError(
            f"portfolio_allocation schema_version mismatch: "
            f"expected={PORTFOLIO_ALLOCATION_CONTRACT_VERSION!r} got={sv!r}"
        )

    b = raw["budget"]
    budget = CapitalBudget(
        total_capital_micros=int(b["total_capital_micros"]),
        max_strategies=int(b["max_strategies"]),
        max_single_strategy_fraction=float(b["max_single_strategy_fraction"]),
    )

    allocations = []
    for a in raw.get("allocations", []):
        allocations.append(
            StrategyAllocation(
                artifact_id=a["artifact_id"],
                status=a["status"],
                allocated_fraction=float(a["allocated_fraction"]),
                allocated_capital_micros=int(a["allocated_capital_micros"]),
                rejection_reason=a.get("rejection_reason"),
                adjustment_reasons=list(a.get("adjustment_reasons", [])),
            )
        )

    return PortfolioAllocationManifest(
        schema_version=raw["schema_version"],
        budget=budget,
        allocations=allocations,
        total_allocated_fraction=float(raw["total_allocated_fraction"]),
        allocated_count=int(raw["allocated_count"]),
        rejected_count=int(raw["rejected_count"]),
        unallocated_capital_micros=int(raw["unallocated_capital_micros"]),
        selection_method=raw["selection_method"],
        produced_at_utc=raw["produced_at_utc"],
    )
