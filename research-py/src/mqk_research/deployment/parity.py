"""
TV-03: Parity evidence builder and writer.

The parity evidence manifest chains the full audit trail:
    artifact_id (TV-01) → gate result (TV-02) → shadow evidence → comparison basis

This module does NOT claim live trust.  live_trust_complete=False is the only
permitted state written by this patch.  All trust gaps are explicit.
"""
from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
from typing import List, Optional

from mqk_research.contracts import (
    DEPLOYABILITY_GATE_CONTRACT_VERSION,
    PARITY_EVIDENCE_CONTRACT_VERSION,
    DeployabilityGateResult,
    ParityEvidenceManifest,
    ShadowEvidenceRef,
)


# ---------------------------------------------------------------------------
# Default live trust gaps.
# These are the explicit gaps that remain before live trust can be complete.
# Each gap maps to a specific future patch or operational requirement.
# ---------------------------------------------------------------------------

_DEFAULT_LIVE_TRUST_GAPS: List[str] = [
    "TV-02 gate evaluates historical metrics only; no live fill data verified",
    "No shadow-mode execution against live broker has been run for this artifact",
    "Live slippage and market-impact costs are not modelled in backtest metrics",
    "Broker execution latency and partial-fill behavior not proven for this artifact",
    "LO-03 operator proof not yet completed; live deployment authorization not issued",
]


def build_parity_evidence(
    *,
    artifact_id: str,
    gate_result: DeployabilityGateResult,
    shadow_evidence: ShadowEvidenceRef,
    comparison_basis: str,
    additional_trust_gaps: Optional[List[str]] = None,
    produced_at_utc: Optional[str] = None,
) -> ParityEvidenceManifest:
    """
    Build a ParityEvidenceManifest linking TV-01 artifact, TV-02 gate result,
    and TV-03 shadow evidence.

    Parameters
    ----------
    artifact_id:
        Canonical artifact ID (TV-01).  Must match gate_result.artifact_id.
    gate_result:
        DeployabilityGateResult from TV-02 evaluation.
    shadow_evidence:
        ShadowEvidenceRef describing available shadow evaluation evidence.
        Use evidence_available=False with a descriptive evidence_note when
        no shadow run has been performed.
    comparison_basis:
        Explicit description of what live-facing assessment this manifest
        is assessed against.  Must be non-empty.
    additional_trust_gaps:
        Optional list of project-specific trust gaps to append to the
        default gaps.  Do not use to suppress default gaps.
    produced_at_utc:
        ISO-8601 UTC string.  Defaults to now if None.  Inject for determinism.

    Returns
    -------
    ParityEvidenceManifest with live_trust_complete=False.

    Raises
    ------
    ValueError: artifact_id is empty, comparison_basis is empty, or
                artifact_id does not match gate_result.artifact_id.
    """
    if not artifact_id:
        raise ValueError("artifact_id must be a non-empty string")
    if not comparison_basis:
        raise ValueError("comparison_basis must be a non-empty string")
    if artifact_id != gate_result.artifact_id:
        raise ValueError(
            f"artifact_id mismatch: manifest artifact_id={artifact_id!r} "
            f"does not match gate_result.artifact_id={gate_result.artifact_id!r}"
        )

    ts = produced_at_utc or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    trust_gaps = list(_DEFAULT_LIVE_TRUST_GAPS)
    if additional_trust_gaps:
        trust_gaps.extend(additional_trust_gaps)

    return ParityEvidenceManifest(
        schema_version=PARITY_EVIDENCE_CONTRACT_VERSION,
        artifact_id=artifact_id,
        gate_passed=gate_result.passed,
        gate_schema_version=gate_result.schema_version,
        shadow_evidence=shadow_evidence,
        comparison_basis=comparison_basis,
        live_trust_complete=False,   # ALWAYS False; set only by LO-03 operator proof
        live_trust_gaps=trust_gaps,
        produced_at_utc=ts,
    )


def write_parity_evidence(artifact_dir: Path, manifest: ParityEvidenceManifest) -> Path:
    """
    Persist the parity evidence manifest to <artifact_dir>/parity_evidence.json.

    Standalone writer — not coupled to promote_signal_pack or to the gate writer.
    Callers supply the artifact_dir (the promoted artifact directory).

    Returns the path written.

    Raises:
        ValueError: artifact_dir does not exist.
    """
    artifact_dir = Path(artifact_dir)
    if not artifact_dir.exists():
        raise ValueError(f"artifact_dir does not exist: {artifact_dir}")

    out = artifact_dir / "parity_evidence.json"
    out.write_text(manifest.to_json(), encoding="utf-8")
    return out
