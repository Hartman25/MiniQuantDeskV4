from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

THIS_DIR = Path(__file__).resolve().parent
if str(THIS_DIR) not in sys.path:
    sys.path.insert(0, str(THIS_DIR))

import run as exp_run


class ExpMeanReversionEngineTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.config_path = Path(__file__).resolve().with_name("config.yaml")
        cls.result = exp_run.run_pipeline(cls.config_path)

    def test_run_id_is_stable(self) -> None:
        config = exp_run._load_config(self.config_path)
        spec = exp_run._to_spec(config)
        bars_path = exp_run._sample_bars_path(spec, self.config_path)
        self.assertEqual(
            exp_run._build_run_id(spec, config_path=self.config_path, bars_path=bars_path),
            self.result.run_id,
        )

    def test_signal_and_trade_counts(self) -> None:
        self.assertEqual(len(self.result.bars), 150)
        self.assertEqual(self.result.metrics["trade_count"], 6)
        self.assertGreater(int((self.result.signals["desired_position"] != 0).sum()), 0)

    def test_metrics_are_stable(self) -> None:
        self.assertAlmostEqual(self.result.metrics["ending_equity"], 1.0498440048760933, places=9)
        self.assertAlmostEqual(self.result.metrics["total_return"], 0.049844004876093306, places=9)
        self.assertAlmostEqual(self.result.metrics["max_drawdown"], -0.019935439516171938, places=9)
        self.assertAlmostEqual(self.result.metrics["profit_factor"], 1.7377380348447335, places=9)
        self.assertAlmostEqual(self.result.metrics["win_rate"], 0.8333333333333334, places=9)

    def test_resolved_config_is_namespaced_and_hashed(self) -> None:
        resolved = self.result.resolved_config
        self.assertEqual(resolved["schema_version"], "exp_config_lock_v1")
        self.assertEqual(resolved["engine"]["engine_id"], "EXP")
        self.assertFalse(resolved["engine"]["canonical"])
        self.assertEqual(resolved["output"]["root_dir"], "runs/EXP/exp_meanrev_v1")
        self.assertEqual(len(resolved["source_files"]["config_sha256"]), 64)
        self.assertEqual(len(resolved["inputs"]["sample_bars_sha256"]), 64)

    def test_write_artifacts_produces_repro_files(self) -> None:
        temp_root = Path(tempfile.mkdtemp(prefix="exp_meanrev_v1_test_"))
        try:
            isolated_result = exp_run.PipelineResult(
                run_id=self.result.run_id,
                run_dir=temp_root / self.result.run_id,
                bars_path=self.result.bars_path,
                config_path=self.result.config_path,
                bars=self.result.bars,
                signals=self.result.signals,
                trades=self.result.trades,
                equity_curve=self.result.equity_curve,
                metrics=self.result.metrics,
                manifest=self.result.manifest,
                resolved_config=self.result.resolved_config,
                artifact_index={},
            )
            exp_run.write_artifacts(isolated_result)

            manifest_path = isolated_result.run_dir / "manifest.json"
            resolved_path = isolated_result.run_dir / "resolved_config.json"
            artifact_index_path = isolated_result.run_dir / "artifact_index.json"
            self.assertTrue(manifest_path.exists())
            self.assertTrue(resolved_path.exists())
            self.assertTrue(artifact_index_path.exists())

            artifact_index = json.loads(artifact_index_path.read_text(encoding="utf-8"))
            self.assertEqual(artifact_index["schema_version"], "exp_artifact_index_v1")
            self.assertEqual(artifact_index["engine_id"], "EXP")
            self.assertEqual(artifact_index["artifact_count"], 6)
            self.assertIn("resolved_config", artifact_index["artifacts"])
            self.assertEqual(
                Path(artifact_index["artifacts"]["resolved_config"]["path"]).name,
                "resolved_config.json",
            )
        finally:
            shutil.rmtree(temp_root)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
