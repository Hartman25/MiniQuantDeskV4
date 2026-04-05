"""
BKT-PROV-02 regression proof — backtest provenance in parity evidence.

Proves that the stronger backtest run identity (BKT-PROV-01) is correctly
threaded through the parity evidence manifest:

BP-01: backtest_run_id and backtest_input_data_hash are stored in the manifest
BP-02: absent backtest_run_id adds an explicit trust gap (fail-closed)
BP-03: present backtest_run_id does NOT add the provenance trust gap
BP-04: fields survive JSON write→read round-trip
BP-05: read_parity_evidence populates both fields (None when absent in JSON)
BP-06: backtest_input_data_hash is independent of backtest_run_id absence logic
       (gap fires only when backtest_run_id is None, regardless of hash presence)
BP-07: live_trust_complete remains False regardless of provenance fields
BP-08: trust gap text names backtest_run_id explicitly (not a vague catch-all)
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from mqk_research.contracts import (
    DEPLOYABILITY_GATE_CONTRACT_VERSION,
    PARITY_EVIDENCE_CONTRACT_VERSION,
    DeployabilityGateResult,
    DeployabilityCheck,
    ParityEvidenceManifest,
    ShadowEvidenceRef,
    read_parity_evidence,
)
from mqk_research.deployment.parity import build_parity_evidence, write_parity_evidence

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

_ARTIFACT_ID = "a" * 64  # 64-char hex string
_BACKTEST_RUN_ID = "12345678-1234-5678-1234-567812345678"
_BACKTEST_INPUT_DATA_HASH = "abcdef01-abcd-ef01-abcd-ef01abcdef01"


def _make_gate_result(artifact_id: str = _ARTIFACT_ID, passed: bool = True) -> DeployabilityGateResult:
    check = DeployabilityCheck(
        name="min_trade_count",
        passed=passed,
        value=200.0,
        threshold=100.0,
        note="Sufficient trades",
    )
    return DeployabilityGateResult(
        schema_version=DEPLOYABILITY_GATE_CONTRACT_VERSION,
        artifact_id=artifact_id,
        passed=passed,
        checks=[check],
        overall_reason="All checks passed" if passed else "Some checks failed",
        evaluated_at_utc="2026-04-01T00:00:00Z",
    )


def _make_shadow_ref() -> ShadowEvidenceRef:
    return ShadowEvidenceRef(
        shadow_label_run_id=None,
        labeled_rows=None,
        precision=None,
        recall=None,
        f1=None,
        evidence_available=False,
        evidence_note="No shadow run performed",
    )


def _build(
    *,
    backtest_run_id=None,
    backtest_input_data_hash=None,
    produced_at_utc="2026-04-01T00:00:00Z",
) -> ParityEvidenceManifest:
    return build_parity_evidence(
        artifact_id=_ARTIFACT_ID,
        gate_result=_make_gate_result(),
        shadow_evidence=_make_shadow_ref(),
        comparison_basis="Swing equity strategy vs historical equity bar data",
        backtest_run_id=backtest_run_id,
        backtest_input_data_hash=backtest_input_data_hash,
        produced_at_utc=produced_at_utc,
    )


# ---------------------------------------------------------------------------
# BP-01: fields stored in manifest
# ---------------------------------------------------------------------------

class TestBacktestProvenanceFields(unittest.TestCase):
    def test_bp01_backtest_run_id_stored(self):
        """backtest_run_id provided → stored in manifest."""
        m = _build(backtest_run_id=_BACKTEST_RUN_ID)
        self.assertEqual(m.backtest_run_id, _BACKTEST_RUN_ID)

    def test_bp01_backtest_input_data_hash_stored(self):
        """backtest_input_data_hash provided → stored in manifest."""
        m = _build(
            backtest_run_id=_BACKTEST_RUN_ID,
            backtest_input_data_hash=_BACKTEST_INPUT_DATA_HASH,
        )
        self.assertEqual(m.backtest_input_data_hash, _BACKTEST_INPUT_DATA_HASH)

    def test_bp01_none_when_not_provided(self):
        """Fields default to None when not passed."""
        m = _build()
        self.assertIsNone(m.backtest_run_id)
        self.assertIsNone(m.backtest_input_data_hash)


# ---------------------------------------------------------------------------
# BP-02: absent backtest_run_id adds trust gap (fail-closed)
# ---------------------------------------------------------------------------

_PROVENANCE_GAP_FRAGMENT = "backtest_run_id"


class TestAbsentProvenanceAddsGap(unittest.TestCase):
    def test_bp02_absent_run_id_adds_trust_gap(self):
        """No backtest_run_id → an explicit trust gap is added."""
        m = _build()
        gap_texts = " ".join(m.live_trust_gaps)
        self.assertIn(
            _PROVENANCE_GAP_FRAGMENT,
            gap_texts,
            f"Expected a trust gap mentioning '{_PROVENANCE_GAP_FRAGMENT}' "
            f"but gaps were: {m.live_trust_gaps!r}",
        )

    def test_bp08_trust_gap_names_backtest_run_id_explicitly(self):
        """BP-08: the provenance gap text names 'backtest_run_id' explicitly."""
        m = _build()
        matching = [g for g in m.live_trust_gaps if "backtest_run_id" in g]
        self.assertTrue(
            matching,
            "At least one trust gap must mention 'backtest_run_id'; "
            f"got gaps: {m.live_trust_gaps!r}",
        )
        # Confirm the text is not a generic catch-all — it must name the missing field.
        self.assertIn("backtest_run_id", matching[0])
        self.assertIn("provenance", matching[0].lower())


# ---------------------------------------------------------------------------
# BP-03: present backtest_run_id does NOT add the provenance gap
# ---------------------------------------------------------------------------

class TestPresentProvenanceNoExtraGap(unittest.TestCase):
    def test_bp03_present_run_id_no_provenance_gap(self):
        """backtest_run_id provided → no backtest_run_id trust gap added."""
        m = _build(backtest_run_id=_BACKTEST_RUN_ID)
        provenance_gaps = [g for g in m.live_trust_gaps if "backtest_run_id" in g]
        self.assertFalse(
            provenance_gaps,
            f"backtest_run_id was provided; expected no provenance trust gap "
            f"but found: {provenance_gaps!r}",
        )

    def test_bp06_hash_alone_does_not_suppress_gap(self):
        """BP-06: providing only backtest_input_data_hash (no run_id) still adds the gap."""
        m = _build(backtest_input_data_hash=_BACKTEST_INPUT_DATA_HASH)
        gap_texts = " ".join(m.live_trust_gaps)
        self.assertIn(
            "backtest_run_id",
            gap_texts,
            "Providing only backtest_input_data_hash must still add the provenance gap "
            "because backtest_run_id is the identity anchor",
        )


# ---------------------------------------------------------------------------
# BP-04: JSON round-trip preserves provenance fields
# ---------------------------------------------------------------------------

class TestRoundTrip(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.mkdtemp()
        self.artifact_dir = Path(self._tmp)

    def tearDown(self):
        import shutil
        shutil.rmtree(self._tmp, ignore_errors=True)

    def test_bp04_fields_survive_json_round_trip(self):
        """write→JSON→read preserves backtest_run_id and backtest_input_data_hash."""
        m = _build(
            backtest_run_id=_BACKTEST_RUN_ID,
            backtest_input_data_hash=_BACKTEST_INPUT_DATA_HASH,
        )
        path = write_parity_evidence(self.artifact_dir, m)
        m2 = read_parity_evidence(path)
        self.assertEqual(m2.backtest_run_id, _BACKTEST_RUN_ID)
        self.assertEqual(m2.backtest_input_data_hash, _BACKTEST_INPUT_DATA_HASH)

    def test_bp04_none_fields_survive_round_trip(self):
        """None provenance fields serialize to null and deserialize to None."""
        m = _build()
        path = write_parity_evidence(self.artifact_dir, m)
        raw = json.loads(path.read_text(encoding="utf-8"))
        self.assertIn("backtest_run_id", raw)
        self.assertIsNone(raw["backtest_run_id"])
        self.assertIn("backtest_input_data_hash", raw)
        self.assertIsNone(raw["backtest_input_data_hash"])
        m2 = read_parity_evidence(path)
        self.assertIsNone(m2.backtest_run_id)
        self.assertIsNone(m2.backtest_input_data_hash)


# ---------------------------------------------------------------------------
# BP-05: read_parity_evidence handles absent fields in legacy JSON gracefully
# ---------------------------------------------------------------------------

class TestLegacyJsonCompat(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.mkdtemp()
        self.artifact_dir = Path(self._tmp)

    def tearDown(self):
        import shutil
        shutil.rmtree(self._tmp, ignore_errors=True)

    def _write_legacy_json(self, extra: dict | None = None) -> Path:
        """Write a parity_evidence.json that lacks the BKT-PROV-02 fields."""
        shadow = {
            "shadow_label_run_id": None,
            "labeled_rows": None,
            "precision": None,
            "recall": None,
            "f1": None,
            "evidence_available": False,
            "evidence_note": "No shadow run",
        }
        raw = {
            "schema_version": PARITY_EVIDENCE_CONTRACT_VERSION,
            "artifact_id": _ARTIFACT_ID,
            "gate_passed": True,
            "gate_schema_version": DEPLOYABILITY_GATE_CONTRACT_VERSION,
            "shadow_evidence": shadow,
            "comparison_basis": "Some basis",
            "live_trust_complete": False,
            "live_trust_gaps": ["gap1"],
            "produced_at_utc": "2026-01-01T00:00:00Z",
            **(extra or {}),
        }
        path = self.artifact_dir / "parity_evidence.json"
        path.write_text(json.dumps(raw, indent=2), encoding="utf-8")
        return path

    def test_bp05_legacy_json_without_provenance_fields_reads_as_none(self):
        """JSON without backtest_run_id/hash → both fields read as None (no error)."""
        path = self._write_legacy_json()
        m = read_parity_evidence(path)
        self.assertIsNone(m.backtest_run_id, "Missing field must deserialize as None")
        self.assertIsNone(m.backtest_input_data_hash, "Missing field must deserialize as None")

    def test_bp05_json_with_null_provenance_fields_reads_as_none(self):
        """JSON with explicit null values → both fields read as None."""
        path = self._write_legacy_json(
            extra={"backtest_run_id": None, "backtest_input_data_hash": None}
        )
        m = read_parity_evidence(path)
        self.assertIsNone(m.backtest_run_id)
        self.assertIsNone(m.backtest_input_data_hash)


# ---------------------------------------------------------------------------
# BP-07: live_trust_complete remains False regardless of provenance fields
# ---------------------------------------------------------------------------

class TestLiveTrustRemainsLocked(unittest.TestCase):
    def test_bp07_live_trust_false_with_full_provenance(self):
        """live_trust_complete is False even when all provenance fields are provided."""
        m = _build(
            backtest_run_id=_BACKTEST_RUN_ID,
            backtest_input_data_hash=_BACKTEST_INPUT_DATA_HASH,
        )
        self.assertFalse(
            m.live_trust_complete,
            "live_trust_complete must remain False regardless of backtest provenance fields",
        )

    def test_bp07_live_trust_false_without_provenance(self):
        """live_trust_complete is False when provenance fields are absent."""
        m = _build()
        self.assertFalse(m.live_trust_complete)


if __name__ == "__main__":
    unittest.main()
