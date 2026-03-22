"""
TV-01: Promoted Artifact Contract tests.

Proves that:
1. derive_artifact_id is stable and deterministic.
2. promote_signal_pack writes a canonical promoted_manifest.json.
3. read_promoted_manifest (consumer reader) successfully consumes the manifest.
4. required_files are all present in the promoted dir.
5. data_root layout is explicit and derivable from artifact_id.
6. artifact_id is stable across identical inputs.
7. lineage captures research-side origin (signal_pack_id, dataset_id, model_id).
8. Producer → consumer end-to-end: same contract, agreed artifact_id.

These tests use no DB and no network.  They create minimal fixtures in a
temporary directory and exercise the full promote → read path.

This is contract-closure proof only.  It does NOT prove strategy viability,
economics, or live deployment readiness.
"""
from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path

import pandas as pd

from mqk_research.contracts import (
    PROMOTED_ARTIFACT_CONTRACT_VERSION,
    SIGNAL_PACK_REQUIRED_FILES,
    PromotedArtifactManifest,
    derive_artifact_id,
    read_promoted_manifest,
)
from mqk_research.ml.util_hash import sha256_json
from mqk_research.signal_pack.promote import promote_signal_pack


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

_MINIMAL_SIGNAL_PACK = {
    "schema_version": "signal_pack_v1",
    "inputs": {},
    "outputs": {},
    "ids": {
        "dataset_id": "ds_test_001",
        "model_id": "mdl_test_001",
        "scores_id": "scores_test_001",
    },
    "compat": {"signal_pack_format": "v1", "signal_column": "signal"},
    "feature_schema_hash": "deadbeef01",
    "model_type": "logreg_v1",
}

_MINIMAL_SIGNAL_PACK_ALT = {
    "schema_version": "signal_pack_v1",
    "inputs": {},
    "outputs": {},
    "ids": {
        "dataset_id": "ds_test_002",
        "model_id": "mdl_test_002",
        "scores_id": "scores_test_002",
    },
    "compat": {"signal_pack_format": "v1", "signal_column": "signal"},
    "feature_schema_hash": "deadbeef02",
    "model_type": "logreg_v1",
}


def _make_run_dir(tmp: Path, sp_content: dict | None = None, signal_rows: int = 5) -> Path:
    """
    Create a minimal run_dir with signal_pack fixtures.

    Layout expected by promote_signal_pack:
        <run_dir>/signal_pack/signal_pack.json
        <run_dir>/signal_pack/signals.csv
    """
    run_dir = tmp / "runs" / "run1"
    sp_dir = run_dir / "signal_pack"
    sp_dir.mkdir(parents=True)

    content = sp_content if sp_content is not None else _MINIMAL_SIGNAL_PACK
    (sp_dir / "signal_pack.json").write_text(
        json.dumps(content, sort_keys=True, separators=(",", ":")),
        encoding="utf-8",
    )

    rows = [
        {
            "symbol": "AAA",
            "end_ts": f"2024-01-{i + 1:02d}T00:00:00Z",
            "signal": round(0.5 + i * 0.01, 4),
            "confidence": 0.5,
        }
        for i in range(signal_rows)
    ]
    pd.DataFrame(rows).to_csv(sp_dir / "signals.csv", index=False)

    return run_dir


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestDeriveArtifactId(unittest.TestCase):
    def test_stable_for_same_inputs(self) -> None:
        """Same artifact_type + source_id → same artifact_id every time."""
        id1 = derive_artifact_id("signal_pack", "abc123deadbeef")
        id2 = derive_artifact_id("signal_pack", "abc123deadbeef")
        self.assertEqual(id1, id2)

    def test_is_hex_string(self) -> None:
        """artifact_id must be a hex string (lowercase)."""
        aid = derive_artifact_id("signal_pack", "abc123")
        self.assertTrue(all(c in "0123456789abcdef" for c in aid), f"not hex: {aid!r}")

    def test_changes_with_source_id(self) -> None:
        """Different source_id → different artifact_id."""
        id1 = derive_artifact_id("signal_pack", "aaa")
        id2 = derive_artifact_id("signal_pack", "bbb")
        self.assertNotEqual(id1, id2)

    def test_changes_with_artifact_type(self) -> None:
        """Different artifact_type → different artifact_id for the same source_id."""
        id1 = derive_artifact_id("signal_pack", "abc123")
        id2 = derive_artifact_id("model", "abc123")
        self.assertNotEqual(id1, id2)


class TestPromoteWritesCanonicalManifest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_promoted_manifest_is_written(self) -> None:
        """promote_signal_pack must write promoted_manifest.json into the dest dir."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        manifest_path = dest / "promoted_manifest.json"
        self.assertTrue(
            manifest_path.exists(),
            "promoted_manifest.json must be written by promote_signal_pack",
        )

    def test_manifest_schema_version_is_canonical(self) -> None:
        """promoted_manifest.json must have schema_version == PROMOTED_ARTIFACT_CONTRACT_VERSION."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        raw = json.loads((dest / "promoted_manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(
            raw["schema_version"],
            PROMOTED_ARTIFACT_CONTRACT_VERSION,
            f"schema_version must be {PROMOTED_ARTIFACT_CONTRACT_VERSION!r}",
        )

    def test_manifest_has_all_required_fields(self) -> None:
        """promoted_manifest.json must contain every required field."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        raw = json.loads((dest / "promoted_manifest.json").read_text(encoding="utf-8"))
        for field in (
            "schema_version",
            "artifact_id",
            "artifact_type",
            "stage",
            "produced_by",
            "data_root",
            "required_files",
            "optional_files",
            "lineage",
            "produced_at_utc",
        ):
            self.assertIn(field, raw, f"promoted_manifest.json missing field: {field!r}")

    def test_stage_and_produced_by_are_canonical(self) -> None:
        """stage must be 'promoted'; produced_by must be 'research-py'."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        raw = json.loads((dest / "promoted_manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(raw["stage"], "promoted")
        self.assertEqual(raw["produced_by"], "research-py")
        self.assertEqual(raw["artifact_type"], "signal_pack")

    def test_required_files_list_matches_constant(self) -> None:
        """required_files in manifest must match SIGNAL_PACK_REQUIRED_FILES."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        raw = json.loads((dest / "promoted_manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(
            sorted(raw["required_files"]),
            sorted(SIGNAL_PACK_REQUIRED_FILES),
            "required_files in manifest must match SIGNAL_PACK_REQUIRED_FILES",
        )

    def test_all_required_files_physically_present(self) -> None:
        """Every file listed in required_files must physically exist in the promoted dir."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        manifest = read_promoted_manifest(dest / "promoted_manifest.json")
        for fname in manifest.required_files:
            self.assertTrue(
                (dest / fname).exists(),
                f"required_file {fname!r} is listed in manifest but does not exist in {dest}",
            )

    def test_data_root_is_posix_and_contains_artifact_id(self) -> None:
        """data_root must be a posix-relative path containing the artifact dir."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        raw = json.loads((dest / "promoted_manifest.json").read_text(encoding="utf-8"))
        data_root = raw["data_root"]
        self.assertNotIn("\\", data_root, "data_root must use forward slashes (posix)")
        self.assertIn("promoted", data_root)
        self.assertIn("signal_packs", data_root)
        # data_root must end with the sp_id directory (same as artifact's parent dir name)
        self.assertTrue(
            data_root.endswith(dest.name) or data_root.endswith("/" + dest.name),
            f"data_root {data_root!r} must end with dest dir name {dest.name!r}",
        )

    def test_lineage_captures_source_ids(self) -> None:
        """lineage must capture signal_pack_id, dataset_id, model_id from signal_pack.json."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        manifest = read_promoted_manifest(dest / "promoted_manifest.json")
        self.assertIsNotNone(manifest.lineage.signal_pack_id)
        self.assertGreater(len(manifest.lineage.signal_pack_id), 0)
        self.assertEqual(manifest.lineage.dataset_id, "ds_test_001")
        self.assertEqual(manifest.lineage.model_id, "mdl_test_001")
        self.assertEqual(manifest.lineage.signal_pack_schema_version, "signal_pack_v1")

    def test_lineage_signal_pack_id_matches_sha256_of_content(self) -> None:
        """lineage.signal_pack_id must equal sha256_json(signal_pack.json content)."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        sp = json.loads(
            (run_dir / "signal_pack" / "signal_pack.json").read_text(encoding="utf-8")
        )
        expected_sp_id = sha256_json(sp)

        manifest = read_promoted_manifest(dest / "promoted_manifest.json")
        self.assertEqual(
            manifest.lineage.signal_pack_id,
            expected_sp_id,
            "lineage.signal_pack_id must equal sha256_json(signal_pack.json)",
        )


class TestArtifactIdDeterminism(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_artifact_id_stable_across_promote_calls(self) -> None:
        """Same signal_pack.json content → same artifact_id on every promote call."""
        run_dir = _make_run_dir(self.tmp)

        dest1 = promote_signal_pack(run_dir, min_rows=1)
        m1 = read_promoted_manifest(dest1 / "promoted_manifest.json")

        # Re-run promote (dest already exists; mkdir exist_ok=True)
        dest2 = promote_signal_pack(run_dir, min_rows=1)
        m2 = read_promoted_manifest(dest2 / "promoted_manifest.json")

        self.assertEqual(dest1, dest2, "Same content must produce same dest dir")
        self.assertEqual(
            m1.artifact_id,
            m2.artifact_id,
            "artifact_id must be stable for identical signal_pack.json content",
        )

    def test_artifact_id_changes_with_content(self) -> None:
        """Different signal_pack.json content → different artifact_id."""
        tmp_a = self.tmp / "a"
        tmp_b = self.tmp / "b"
        tmp_a.mkdir()
        tmp_b.mkdir()

        run_dir_a = _make_run_dir(tmp_a, sp_content=_MINIMAL_SIGNAL_PACK)
        run_dir_b = _make_run_dir(tmp_b, sp_content=_MINIMAL_SIGNAL_PACK_ALT)

        dest_a = promote_signal_pack(run_dir_a, min_rows=1)
        dest_b = promote_signal_pack(run_dir_b, min_rows=1)

        m_a = read_promoted_manifest(dest_a / "promoted_manifest.json")
        m_b = read_promoted_manifest(dest_b / "promoted_manifest.json")

        self.assertNotEqual(
            m_a.artifact_id,
            m_b.artifact_id,
            "Different signal_pack.json content must produce different artifact_ids",
        )


class TestConsumerReadsContract(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_read_promoted_manifest_returns_correct_type(self) -> None:
        """read_promoted_manifest must return a PromotedArtifactManifest instance."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        manifest = read_promoted_manifest(dest / "promoted_manifest.json")
        self.assertIsInstance(manifest, PromotedArtifactManifest)

    def test_read_promoted_manifest_validates_schema_version(self) -> None:
        """read_promoted_manifest must raise ValueError on wrong schema_version."""
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        # Tamper with schema_version
        manifest_path = dest / "promoted_manifest.json"
        raw = json.loads(manifest_path.read_text(encoding="utf-8"))
        raw["schema_version"] = "promoted-v0"
        manifest_path.write_text(json.dumps(raw), encoding="utf-8")

        with self.assertRaises(ValueError) as ctx:
            read_promoted_manifest(manifest_path)
        self.assertIn("schema_version mismatch", str(ctx.exception))

    def test_read_promoted_manifest_raises_on_missing_file(self) -> None:
        """read_promoted_manifest must raise FileNotFoundError for missing manifest."""
        with self.assertRaises(FileNotFoundError):
            read_promoted_manifest(self.tmp / "no_such_dir" / "promoted_manifest.json")

    def test_producer_consumer_artifact_id_agree(self) -> None:
        """
        End-to-end: producer writes manifest with artifact_id;
        consumer reads it back and gets the same artifact_id.
        Both agree on the same contract — the handoff is closed.
        """
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        # Consumer reads the manifest
        manifest = read_promoted_manifest(dest / "promoted_manifest.json")

        # Independently derive what the artifact_id should be
        sp = json.loads(
            (run_dir / "signal_pack" / "signal_pack.json").read_text(encoding="utf-8")
        )
        sp_id = sha256_json(sp)
        expected_artifact_id = derive_artifact_id("signal_pack", sp_id)

        self.assertEqual(
            manifest.artifact_id,
            expected_artifact_id,
            "Producer and consumer must agree on artifact_id derived from signal_pack content",
        )
        self.assertEqual(manifest.schema_version, PROMOTED_ARTIFACT_CONTRACT_VERSION)
        self.assertEqual(manifest.stage, "promoted")
        self.assertEqual(manifest.artifact_type, "signal_pack")

    def test_consumer_can_locate_data_root_from_artifact_id(self) -> None:
        """
        data_root in the manifest lets the consumer locate the artifact directory
        without needing to guess the path.

        Layout rule: <project_root>/<data_root> is the artifact directory.
        """
        run_dir = _make_run_dir(self.tmp)
        dest = promote_signal_pack(run_dir, min_rows=1)

        manifest = read_promoted_manifest(dest / "promoted_manifest.json")

        # project_root = run_dir.parents[1] (same derivation as promote_signal_pack)
        project_root = run_dir.parents[1]
        resolved = project_root / Path(manifest.data_root)

        self.assertEqual(
            resolved.resolve(),
            dest.resolve(),
            "project_root / data_root must resolve to the promoted artifact directory",
        )


if __name__ == "__main__":
    unittest.main()
