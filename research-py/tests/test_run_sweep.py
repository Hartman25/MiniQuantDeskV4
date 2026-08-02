"""
RESEARCH-PY-RUN-SWEEP-SYNTAX-FIX-01 — Unit tests for the sweep-plan scaffold.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from mqk_research.sweeps.run_sweep import run_sweep_scaffold


class TestRunSweepScaffold(unittest.TestCase):
    def test_malformed_grid_json_raises_bounded_value_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            grid_path = Path(tmp) / "grid.json"
            grid_path.write_text(json.dumps({"grid": {"a": []}}), encoding="utf-8")
            out_csv = Path(tmp) / "sweep_plan.csv"

            with self.assertRaises(ValueError) as ctx:
                run_sweep_scaffold(grid_json=grid_path, out_csv=out_csv)

            self.assertIn("grid_json must include", str(ctx.exception))
            self.assertFalse(out_csv.exists())

    def test_valid_grid_writes_sweep_plan_csv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            grid_path = Path(tmp) / "grid.json"
            grid_path.write_text(
                json.dumps({"grid": {"a": [1, 2], "b": ["x"]}}), encoding="utf-8"
            )
            out_csv = Path(tmp) / "sweep_plan.csv"

            result = run_sweep_scaffold(grid_json=grid_path, out_csv=out_csv)

            self.assertEqual(result, out_csv)
            self.assertTrue(out_csv.exists())
            content = out_csv.read_text(encoding="utf-8")
            self.assertIn("sweep_id", content)
            self.assertIn("a", content)
            self.assertIn("b", content)
            # 2 values for "a" x 1 value for "b" = 2 rows + header.
            self.assertEqual(len(content.strip().splitlines()), 3)


if __name__ == "__main__":
    unittest.main()
