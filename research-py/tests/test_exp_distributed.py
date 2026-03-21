from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

import pandas as pd

from mqk_research.exp_distributed.hashing import stable_hash
from mqk_research.exp_distributed.models import BatchSpec, WindowSpec
from mqk_research.exp_distributed.runner import (
    batch_summary,
    create_batch,
    default_root,
    failed_jobs,
    rerun_failed_jobs,
    run_batch,
)
from mqk_research.exp_distributed.scheduler import build_batch_plan
from mqk_research.exp_distributed.storage import ResearchResultStore


class ExpDistributedTests(unittest.TestCase):
    def _write_dataset(self, root: Path) -> Path:
        rows = []
        base_dates = pd.date_range("2024-01-01", periods=12, freq="D", tz="UTC")
        for idx, ts in enumerate(base_dates):
            rows.append({"symbol": "AAA", "timeframe": "1D", "end_ts": int(ts.timestamp()), "close": 100 + idx})
            rows.append({"symbol": "BBB", "timeframe": "1D", "end_ts": int(ts.timestamp()), "close": 100 + idx * 2})
            rows.append({"symbol": "CCC", "timeframe": "1D", "end_ts": int(ts.timestamp()), "close": 100 + max(0, 6 - idx)})
        path = root / "sample.csv"
        pd.DataFrame(rows).to_csv(path, index=False)
        return path

    def _write_batch_spec(self, root: Path, dataset_path: Path) -> Path:
        payload = {
            "schema_version": "exp-distributed-v1",
            "engine_id": "EXP",
            "experiment_id": "exp.momentum_smoke",
            "batch_label": "unit-test-sweep",
            "dataset_path": str(dataset_path),
            "strategy_id": "exp.cross_sectional_momentum_v1",
            "timeframe": "1D",
            "base_params": {"min_signal": 0.0, "rebalance_every": 1},
            "parameter_grid": {"lookback_days": [2, 3], "top_n": [1, 2]},
            "symbol_groups": [["AAA", "BBB"], ["AAA", "CCC"]],
            "windows": [
                {"label": "window_a", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"},
            ],
            "max_workers": 1,
            "notes": ["test only"],
        }
        path = root / "batch.json"
        path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
        return path

    def test_hashing_is_stable(self) -> None:
        left = {"b": [2, 1], "a": {"z": 1, "y": 2}}
        right = {"a": {"y": 2, "z": 1}, "b": [2, 1]}
        self.assertEqual(stable_hash(left), stable_hash(right))

    def test_scheduler_expands_deterministically(self) -> None:
        spec = BatchSpec(
            experiment_id="exp.test",
            dataset_path="/tmp/data.csv",
            strategy_id="exp.cross_sectional_momentum_v1",
            windows=[WindowSpec(start_utc="2024-01-01T00:00:00Z", end_utc="2024-01-10T00:00:00Z", label="w1")],
            symbol_groups=[["BBB", "AAA"], ["CCC"]],
            base_params={"rebalance_every": 1},
            parameter_grid={"lookback_days": [2, 3], "top_n": [1]},
        )
        with tempfile.TemporaryDirectory() as temp_dir:
            dataset_path = self._write_dataset(Path(temp_dir))
            spec = BatchSpec.from_dict(spec.to_dict() | {"dataset_path": str(dataset_path)})
            plan = build_batch_plan(spec)
        self.assertEqual(len(plan.jobs), 4)
        self.assertTrue(all(job.engine_id == "EXP" for job in plan.jobs))
        self.assertEqual(plan.jobs[0].symbols, ["AAA", "BBB"])
        self.assertEqual(plan.jobs[0].params["lookback_days"], 2)
        self.assertEqual(plan.jobs[1].params["lookback_days"], 3)

    def test_run_batch_writes_artifacts_and_summary(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            dataset_path = self._write_dataset(root)
            batch_spec_path = self._write_batch_spec(root, dataset_path)
            result = run_batch(batch_spec_path, root=root / "research_out", max_workers=1)
            self.assertEqual(result["status"], "succeeded")
            summary = result["summary"]
            self.assertEqual(summary["job_count"], 8)
            self.assertEqual(summary["failed"], 0)
            leaderboard_path = Path(result["aggregate_paths"]["leaderboard"])
            self.assertTrue(leaderboard_path.exists())
            self.assertIn("exp_distributed", str(leaderboard_path))
            batch_state = batch_summary(result["batch_id"], root=root / "research_out")
            self.assertEqual(batch_state["summary"]["job_count"], 8)

    def test_storage_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            store = ResearchResultStore(Path(temp_dir) / "state" / "exp.sqlite3")
            store.upsert_batch(
                batch_id="batch-1",
                spec={"engine_id": "EXP", "experiment_id": "exp.test", "strategy_id": "exp.buy_hold_v1", "batch_label": "b1"},
                root_dir=Path(temp_dir),
                job_count=0,
            )
            batch = store.get_batch("batch-1")
            self.assertEqual(batch["engine_id"], "EXP")

    def test_failed_job_can_be_inspected(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            dataset_path = self._write_dataset(root)
            spec_payload = {
                "schema_version": "exp-distributed-v1",
                "engine_id": "EXP",
                "experiment_id": "exp.failcase",
                "dataset_path": str(dataset_path),
                "strategy_id": "exp.cross_sectional_momentum_v1",
                "timeframe": "1D",
                "base_params": {},
                "parameter_grid": {"lookback_days": [0], "top_n": [1]},
                "symbol_groups": [["AAA", "BBB"]],
                "windows": [{"label": "bad", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}],
                "max_workers": 1,
            }
            spec_path = root / "bad_batch.json"
            spec_path.write_text(json.dumps(spec_payload, indent=2), encoding="utf-8")
            result = run_batch(spec_path, root=root / "research_out", max_workers=1)
            self.assertEqual(result["status"], "failed")
            failed = failed_jobs(result["batch_id"], root=root / "research_out")
            self.assertEqual(len(failed["failed_jobs"]), 1)
            self.assertIn("lookback_days", Path(failed["failed_jobs"][0]["job_spec_path"]).read_text(encoding="utf-8"))

    def test_relative_dataset_path_resolves_against_spec_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            spec_dir = root / "specs"
            data_dir = root / "data"
            spec_dir.mkdir(parents=True, exist_ok=True)
            data_dir.mkdir(parents=True, exist_ok=True)
            dataset_path = self._write_dataset(data_dir)
            spec_payload = {
                "schema_version": "exp-distributed-v1",
                "engine_id": "EXP",
                "experiment_id": "exp.relative_path",
                "dataset_path": "../data/sample.csv",
                "strategy_id": "exp.cross_sectional_momentum_v1",
                "timeframe": "1D",
                "base_params": {"rebalance_every": 1},
                "parameter_grid": {"lookback_days": [2], "top_n": [1]},
                "symbol_groups": [["AAA", "BBB"]],
                "windows": [{"label": "ok", "start_utc": "2024-01-01T00:00:00Z", "end_utc": "2024-01-12T00:00:00Z"}],
                "max_workers": 1,
            }
            spec_path = spec_dir / "batch.json"
            spec_path.write_text(json.dumps(spec_payload, indent=2), encoding="utf-8")

            original_cwd = Path.cwd()
            os.chdir(root)
            try:
                created = create_batch(spec_path)
            finally:
                os.chdir(original_cwd)

            self.assertEqual(created["job_count"], 1)
            self.assertEqual(Path(created["root"]), default_root())
            job_spec_path = Path(next(iter(created["job_spec_paths"].values())))
            persisted_spec = json.loads(job_spec_path.read_text(encoding="utf-8"))
            self.assertEqual(Path(persisted_spec["dataset_fingerprint"]["dataset_path"]), dataset_path.resolve())

    def test_rerun_failed_jobs_returns_no_failed_jobs_when_clean(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            dataset_path = self._write_dataset(root)
            batch_spec_path = self._write_batch_spec(root, dataset_path)
            result = run_batch(batch_spec_path, root=root / "research_out", max_workers=1)
            rerun = rerun_failed_jobs(result["batch_id"], root=root / "research_out", max_workers=1)
            self.assertEqual(rerun["status"], "no_failed_jobs")
            self.assertEqual(rerun["rerun_count"], 0)


if __name__ == "__main__":
    unittest.main()
