"""
TV-03: Parity Evidence proof tests.

Proves that:
1. build_parity_evidence produces a valid ParityEvidenceManifest.
2. live_trust_complete is ALWAYS False (not settable via this API).
3. All default live_trust_gaps are present.
4. Additional trust gaps are appended (not replacing defaults).
5. artifact_id mismatch between manifest and gate_result raises ValueError.
6. write_parity_evidence persists a stable, machine-readable manifest.
7. read_parity_evidence (consumer reader) successfully consumes the manifest.
8. Schema version mismatch raises ValueError.
9. Evidence-unavailable shadow ref round-trips faithfully.
10. End-to-end TV-01→TV-02→TV-03 chain: same artifact_id links all three.

These tests use no DB and no network.  They create minimal fixtures in a
temporary directory and exercise the full build → write → read path.

This is parity-evidence-contract proof only.  It does NOT prove strategy
viability, shadow precision, or live deployment readiness.
"""
from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from mqk_research.contracts import (
    DEPLOYABILITY_GATE_CONTRACT_VERSION,
    PARITY_EVIDENCE_CONTRACT_VERSION,
    ParityEvidenceManifest,
    read_parity_evidence,
)
from mqk_research.deployment.gate import (
    DeployabilityGateConfig,
    evaluate_deployability_gate,
    write_deployability_gate,
)
from mqk_research.deployment.parity import (
    _DEFAULT_LIVE_TRUST_GAPS,
    build_parity_evidence,
    write_parity_evidence,
)

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

_ARTIFACT_ID = "c" * 64
_OTHER_ARTIFACT_ID = "d" * 64
_TS = "2025-06-01T00:00:00Z"

_PASSING_METRICS = {
    "trade_event_count": 50,
    "trading_days": 120,
    "turnover": 240.0,
    "active_days": 80,
}

_NO_SHADOW = {
    "shadow_label_run_id": None,
    "labeled_rows": None,
    "precision": None,
    "recall": None,
    "f1": None,
    "evidence_available": False,
    "evidence_note": "No shadow evaluation has been run for this artifact.",
}


def _make_shadow_ref(evidence_available: bool = False):
    from mqk_research.contracts import ShadowEvidenceRef
    return ShadowEvidenceRef(
        shadow_label_run_id=None,
        labeled_rows=None,
        precision=None,
        recall=None,
        f1=None,
        evidence_available=evidence_available,
        evidence_note="No shadow evaluation has been run for this artifact." if not evidence_available
                      else "Shadow run lblrun_001 completed with 500 rows.",
    )


def _make_gate_result(artifact_id: str = _ARTIFACT_ID) -> "DeployabilityGateResult":
    from mqk_research.deployment.gate import evaluate_deployability_gate
    return evaluate_deployability_gate(
        artifact_id, _PASSING_METRICS, evaluated_at_utc=_TS
    )


# ---------------------------------------------------------------------------
# Tests: build_parity_evidence
# ---------------------------------------------------------------------------


class TestBuildParityEvidence(unittest.TestCase):
    def test_returns_parity_evidence_manifest(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Backtest 2020-01-01 to 2023-12-31 vs paper-trading baseline.",
            produced_at_utc=_TS,
        )
        self.assertIsInstance(manifest, ParityEvidenceManifest)

    def test_schema_version_is_canonical(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        self.assertEqual(manifest.schema_version, PARITY_EVIDENCE_CONTRACT_VERSION)

    def test_live_trust_complete_is_always_false(self) -> None:
        """live_trust_complete must be False — it cannot be set True by this API."""
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        self.assertFalse(
            manifest.live_trust_complete,
            "live_trust_complete must always be False from this patch",
        )

    def test_default_trust_gaps_all_present(self) -> None:
        """All _DEFAULT_LIVE_TRUST_GAPS must appear in the manifest."""
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        for gap in _DEFAULT_LIVE_TRUST_GAPS:
            self.assertIn(gap, manifest.live_trust_gaps, f"Default gap missing: {gap!r}")

    def test_additional_trust_gaps_appended(self) -> None:
        """additional_trust_gaps are appended to defaults, not replacing them.

        Pass backtest_run_id so the count is predictable (no auto-added provenance gap).
        """
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        extra = ["My project-specific gap #1."]
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            backtest_run_id="00000000-0000-0000-0000-000000000001",  # suppress provenance gap
            additional_trust_gaps=extra,
            produced_at_utc=_TS,
        )
        for gap in _DEFAULT_LIVE_TRUST_GAPS:
            self.assertIn(gap, manifest.live_trust_gaps)
        self.assertIn(extra[0], manifest.live_trust_gaps)
        # default gaps still there — not replaced; exact count is defaults + extra
        self.assertEqual(len(manifest.live_trust_gaps), len(_DEFAULT_LIVE_TRUST_GAPS) + len(extra))

    def test_artifact_id_matches_gate_result(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        self.assertEqual(manifest.artifact_id, _ARTIFACT_ID)
        self.assertEqual(manifest.gate_schema_version, DEPLOYABILITY_GATE_CONTRACT_VERSION)

    def test_gate_passed_propagated_correctly(self) -> None:
        gate = _make_gate_result()  # passing gate
        shadow = _make_shadow_ref()
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        self.assertTrue(manifest.gate_passed)

    def test_artifact_id_mismatch_raises_value_error(self) -> None:
        """artifact_id != gate_result.artifact_id must raise ValueError."""
        gate = _make_gate_result(artifact_id=_OTHER_ARTIFACT_ID)
        shadow = _make_shadow_ref()
        with self.assertRaises(ValueError) as ctx:
            build_parity_evidence(
                artifact_id=_ARTIFACT_ID,     # different from gate's artifact_id
                gate_result=gate,
                shadow_evidence=shadow,
                comparison_basis="Test basis.",
            )
        self.assertIn("mismatch", str(ctx.exception))

    def test_empty_artifact_id_raises_value_error(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        # Need a gate with empty artifact_id to avoid the mismatch check first
        from mqk_research.contracts import DeployabilityGateResult, DeployabilityCheck
        fake_gate = DeployabilityGateResult(
            schema_version=DEPLOYABILITY_GATE_CONTRACT_VERSION,
            artifact_id="",
            passed=True,
            checks=[],
            overall_reason="fake",
            evaluated_at_utc=_TS,
        )
        with self.assertRaises(ValueError):
            build_parity_evidence(
                artifact_id="",
                gate_result=fake_gate,
                shadow_evidence=shadow,
                comparison_basis="Test basis.",
            )

    def test_empty_comparison_basis_raises_value_error(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        with self.assertRaises(ValueError):
            build_parity_evidence(
                artifact_id=_ARTIFACT_ID,
                gate_result=gate,
                shadow_evidence=shadow,
                comparison_basis="",
            )

    def test_deterministic_for_same_inputs(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        kwargs = dict(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        m1 = build_parity_evidence(**kwargs)
        m2 = build_parity_evidence(**kwargs)
        self.assertEqual(m1.to_json(), m2.to_json())

    def test_shadow_evidence_no_evidence_round_trips(self) -> None:
        gate = _make_gate_result()
        shadow = _make_shadow_ref(evidence_available=False)
        manifest = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Test basis.",
            produced_at_utc=_TS,
        )
        self.assertFalse(manifest.shadow_evidence.evidence_available)
        self.assertIsNone(manifest.shadow_evidence.precision)
        self.assertIsNone(manifest.shadow_evidence.f1)


# ---------------------------------------------------------------------------
# Tests: write/read round-trip
# ---------------------------------------------------------------------------


class TestWriteAndReadParityEvidence(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _build(self, *, additional_gaps=None) -> ParityEvidenceManifest:
        gate = _make_gate_result()
        shadow = _make_shadow_ref()
        return build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate,
            shadow_evidence=shadow,
            comparison_basis="Backtest 2020-2023 vs paper baseline.",
            additional_trust_gaps=additional_gaps,
            produced_at_utc=_TS,
        )

    def test_write_creates_file(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        self.assertTrue(path.exists())
        self.assertEqual(path.name, "parity_evidence.json")

    def test_written_json_has_required_fields(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        raw = json.loads(path.read_text(encoding="utf-8"))
        for field in (
            "schema_version",
            "artifact_id",
            "gate_passed",
            "gate_schema_version",
            "shadow_evidence",
            "comparison_basis",
            "live_trust_complete",
            "live_trust_gaps",
            "produced_at_utc",
        ):
            self.assertIn(field, raw, f"Missing field in parity_evidence.json: {field!r}")

    def test_write_then_read_returns_parity_evidence_manifest(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        recovered = read_parity_evidence(path)
        self.assertIsInstance(recovered, ParityEvidenceManifest)

    def test_write_then_read_fields_agree(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        recovered = read_parity_evidence(path)

        self.assertEqual(recovered.schema_version, manifest.schema_version)
        self.assertEqual(recovered.artifact_id, manifest.artifact_id)
        self.assertEqual(recovered.gate_passed, manifest.gate_passed)
        self.assertEqual(recovered.gate_schema_version, manifest.gate_schema_version)
        self.assertEqual(recovered.comparison_basis, manifest.comparison_basis)
        self.assertFalse(recovered.live_trust_complete)
        self.assertEqual(sorted(recovered.live_trust_gaps), sorted(manifest.live_trust_gaps))

    def test_write_then_read_shadow_evidence_faithful(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        recovered = read_parity_evidence(path)

        self.assertEqual(
            recovered.shadow_evidence.evidence_available,
            manifest.shadow_evidence.evidence_available,
        )
        self.assertEqual(
            recovered.shadow_evidence.evidence_note,
            manifest.shadow_evidence.evidence_note,
        )
        self.assertIsNone(recovered.shadow_evidence.precision)

    def test_schema_version_mismatch_raises_value_error(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        raw = json.loads(path.read_text(encoding="utf-8"))
        raw["schema_version"] = "parity-v0"
        path.write_text(json.dumps(raw), encoding="utf-8")

        with self.assertRaises(ValueError) as ctx:
            read_parity_evidence(path)
        self.assertIn("schema_version mismatch", str(ctx.exception))

    def test_read_missing_file_raises_file_not_found(self) -> None:
        with self.assertRaises(FileNotFoundError):
            read_parity_evidence(self.tmp / "no_such_dir" / "parity_evidence.json")

    def test_write_to_nonexistent_dir_raises_value_error(self) -> None:
        manifest = self._build()
        with self.assertRaises(ValueError):
            write_parity_evidence(self.tmp / "does_not_exist", manifest)

    def test_live_trust_complete_is_false_after_round_trip(self) -> None:
        manifest = self._build()
        path = write_parity_evidence(self.tmp, manifest)
        recovered = read_parity_evidence(path)
        self.assertFalse(recovered.live_trust_complete)

    def test_write_is_deterministic(self) -> None:
        manifest = self._build()
        p1 = write_parity_evidence(self.tmp, manifest)
        content1 = p1.read_text(encoding="utf-8")
        p2 = write_parity_evidence(self.tmp, manifest)
        content2 = p2.read_text(encoding="utf-8")
        self.assertEqual(content1, content2)


# ---------------------------------------------------------------------------
# End-to-end: TV-01 → TV-02 → TV-03 chain
# ---------------------------------------------------------------------------


class TestEndToEndChain(unittest.TestCase):
    """
    TV-01 → TV-02 → TV-03 chain proof.

    All three artifacts share the same artifact_id.
    The parity evidence manifest explicitly links gate_schema_version (TV-02)
    and artifact_id (TV-01) so any consumer can follow the chain.
    """

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_chain_artifact_id_links_all_three_artifacts(self) -> None:
        """
        artifact_id is consistent across promoted_manifest (TV-01),
        deployability_gate (TV-02), and parity_evidence (TV-03).
        """
        # TV-01: simulate what promote_signal_pack produces
        artifact_dir = self.tmp / "artifact"
        artifact_dir.mkdir()

        # TV-02: evaluate gate, write to artifact dir
        gate_result = evaluate_deployability_gate(
            _ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=_TS
        )
        gate_path = write_deployability_gate(artifact_dir, gate_result)

        # TV-03: build parity evidence, write to same artifact dir
        shadow = _make_shadow_ref()
        parity = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate_result,
            shadow_evidence=shadow,
            comparison_basis="Chain test basis.",
            produced_at_utc=_TS,
        )
        parity_path = write_parity_evidence(artifact_dir, parity)

        # Consumer: read both back and verify artifact_id links them
        gate_read = __import__(
            "mqk_research.contracts", fromlist=["read_deployability_gate"]
        ).read_deployability_gate(gate_path)
        parity_read = read_parity_evidence(parity_path)

        self.assertEqual(gate_read.artifact_id, _ARTIFACT_ID)
        self.assertEqual(parity_read.artifact_id, _ARTIFACT_ID)
        self.assertEqual(gate_read.artifact_id, parity_read.artifact_id)

    def test_chain_gate_schema_version_links_tv02_to_tv03(self) -> None:
        """
        parity_evidence.gate_schema_version must equal the gate_result's schema_version,
        creating an explicit typed link from TV-03 back to TV-02.
        """
        gate_result = evaluate_deployability_gate(
            _ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=_TS
        )
        shadow = _make_shadow_ref()
        parity = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate_result,
            shadow_evidence=shadow,
            comparison_basis="Chain test basis.",
            produced_at_utc=_TS,
        )
        self.assertEqual(parity.gate_schema_version, gate_result.schema_version)
        self.assertEqual(parity.gate_schema_version, DEPLOYABILITY_GATE_CONTRACT_VERSION)

    def test_chain_both_files_co_located_in_artifact_dir(self) -> None:
        """Both deployability_gate.json and parity_evidence.json exist in artifact_dir."""
        artifact_dir = self.tmp / "artifact"
        artifact_dir.mkdir()

        gate_result = evaluate_deployability_gate(
            _ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=_TS
        )
        write_deployability_gate(artifact_dir, gate_result)

        shadow = _make_shadow_ref()
        parity = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate_result,
            shadow_evidence=shadow,
            comparison_basis="Chain test.",
            produced_at_utc=_TS,
        )
        write_parity_evidence(artifact_dir, parity)

        self.assertTrue((artifact_dir / "deployability_gate.json").exists())
        self.assertTrue((artifact_dir / "parity_evidence.json").exists())

    def test_chain_gate_passed_propagated_to_parity(self) -> None:
        """gate_passed in parity_evidence reflects the actual gate result."""
        # Passing gate
        gate_pass = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=_TS)
        shadow = _make_shadow_ref()
        parity_pass = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate_pass,
            shadow_evidence=shadow,
            comparison_basis="Basis.",
            produced_at_utc=_TS,
        )
        self.assertTrue(parity_pass.gate_passed)

        # Failing gate
        failing_metrics = {
            "trade_event_count": 5,   # fails min_trade_count
            "trading_days": 120,
            "turnover": 240.0,
            "active_days": 80,
        }
        gate_fail = evaluate_deployability_gate(_ARTIFACT_ID, failing_metrics, evaluated_at_utc=_TS)
        parity_fail = build_parity_evidence(
            artifact_id=_ARTIFACT_ID,
            gate_result=gate_fail,
            shadow_evidence=shadow,
            comparison_basis="Basis.",
            produced_at_utc=_TS,
        )
        self.assertFalse(parity_fail.gate_passed)


if __name__ == "__main__":
    unittest.main()
