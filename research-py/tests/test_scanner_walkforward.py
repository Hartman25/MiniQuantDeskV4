"""
WALKFORWARD-SPLIT-01 — Tests for walk-forward date split builder.

Tests build_date_splits is deterministic, correct, and fail-closed.
No subprocess, no broker, no OMS, no network, no DB.
"""
from __future__ import annotations

import unittest
from datetime import date

from mqk_research.scanner.walkforward import (
    WalkForwardConfig,
    WalkForwardSplit,
    build_date_splits,
)


class TestBuildDateSplitsBasic(unittest.TestCase):

    def test_single_fold_fits_in_window(self):
        """One fold when window is exactly train+validation days."""
        cfg = WalkForwardConfig(train_days=70, validation_days=30, step_days=30)
        start = date(2025, 1, 1)
        end = date(2025, 4, 10)   # 99 days span — train 70 + validation 30 = 100 (1-indexed)
        splits = build_date_splits(start, end, cfg)
        self.assertGreaterEqual(len(splits), 1)
        s = splits[0]
        self.assertEqual(s.fold_index, 0)
        self.assertEqual(s.train_start, start)
        self.assertEqual(s.train_days, 70)
        self.assertEqual(s.validation_days, 30)

    def test_split_windows_are_contiguous(self):
        """validation_start immediately follows train_end (no gap)."""
        cfg = WalkForwardConfig(train_days=60, validation_days=20, step_days=20)
        start = date(2025, 1, 1)
        end = date(2025, 6, 30)
        splits = build_date_splits(start, end, cfg)
        self.assertGreater(len(splits), 0)
        for s in splits:
            expected_vs = s.train_end + __import__("datetime").timedelta(days=1)
            self.assertEqual(s.validation_start, expected_vs)

    def test_folds_advance_by_step_days(self):
        """Each fold starts step_days later than the previous."""
        from datetime import timedelta
        cfg = WalkForwardConfig(train_days=60, validation_days=20, step_days=30)
        start = date(2025, 1, 1)
        end = date(2025, 12, 31)
        splits = build_date_splits(start, end, cfg)
        self.assertGreater(len(splits), 1)
        for i in range(1, len(splits)):
            delta = (splits[i].train_start - splits[i - 1].train_start).days
            self.assertEqual(delta, 30)

    def test_no_fold_extends_beyond_end_date(self):
        """validation_end <= end_date for all folds."""
        cfg = WalkForwardConfig(train_days=60, validation_days=20, step_days=20)
        end = date(2025, 6, 30)
        splits = build_date_splits(date(2025, 1, 1), end, cfg)
        for s in splits:
            self.assertLessEqual(s.validation_end, end)

    def test_empty_list_when_window_too_short(self):
        """Empty list when train+validation exceeds available days."""
        cfg = WalkForwardConfig(train_days=200, validation_days=100, step_days=50)
        start = date(2025, 1, 1)
        end = date(2025, 3, 1)   # only ~59 days
        splits = build_date_splits(start, end, cfg)
        self.assertEqual(splits, [])

    def test_fold_index_increments(self):
        """fold_index is 0-based and strictly incrementing."""
        cfg = WalkForwardConfig(train_days=30, validation_days=10, step_days=10)
        start = date(2025, 1, 1)
        end = date(2025, 12, 31)
        splits = build_date_splits(start, end, cfg)
        for i, s in enumerate(splits):
            self.assertEqual(s.fold_index, i)

    def test_default_config_used_when_none_passed(self):
        """None config falls back to default WalkForwardConfig."""
        start = date(2025, 1, 1)
        end = date(2026, 12, 31)
        splits = build_date_splits(start, end, None)
        self.assertGreater(len(splits), 0)


class TestBuildDateSplitsDeterminism(unittest.TestCase):

    def test_same_inputs_produce_same_splits(self):
        """Two calls with identical inputs produce identical fold sequences."""
        cfg = WalkForwardConfig(train_days=60, validation_days=20, step_days=20)
        start = date(2025, 3, 1)
        end = date(2025, 12, 31)
        splits_a = build_date_splits(start, end, cfg)
        splits_b = build_date_splits(start, end, cfg)
        self.assertEqual(len(splits_a), len(splits_b))
        for a, b in zip(splits_a, splits_b):
            self.assertEqual(a.train_start, b.train_start)
            self.assertEqual(a.train_end, b.train_end)
            self.assertEqual(a.validation_start, b.validation_start)
            self.assertEqual(a.validation_end, b.validation_end)

    def test_different_start_dates_produce_different_splits(self):
        """Different start_dates produce different first train_start."""
        cfg = WalkForwardConfig(train_days=60, validation_days=20, step_days=20)
        end = date(2025, 12, 31)
        splits_a = build_date_splits(date(2025, 1, 1), end, cfg)
        splits_b = build_date_splits(date(2025, 2, 1), end, cfg)
        self.assertNotEqual(splits_a[0].train_start, splits_b[0].train_start)


class TestBuildDateSplitsMinimums(unittest.TestCase):

    def test_folds_excluded_when_train_below_minimum(self):
        """Config with min_train_days larger than actual train window → empty."""
        cfg = WalkForwardConfig(
            train_days=10,
            validation_days=5,
            step_days=5,
            min_train_days=30,  # higher than train_days
            min_validation_days=3,
        )
        splits = build_date_splits(date(2025, 1, 1), date(2025, 12, 31), cfg)
        self.assertEqual(splits, [])

    def test_folds_excluded_when_validation_below_minimum(self):
        """Config with min_validation_days larger than validation_days → empty."""
        cfg = WalkForwardConfig(
            train_days=30,
            validation_days=5,
            step_days=10,
            min_train_days=10,
            min_validation_days=20,  # higher than validation_days
        )
        splits = build_date_splits(date(2025, 1, 1), date(2025, 12, 31), cfg)
        self.assertEqual(splits, [])


class TestWalkForwardSplitProperties(unittest.TestCase):

    def test_train_days_property_correct(self):
        """WalkForwardSplit.train_days returns inclusive day count."""
        s = WalkForwardSplit(
            fold_index=0,
            train_start=date(2025, 1, 1),
            train_end=date(2025, 3, 11),    # 70 days
            validation_start=date(2025, 3, 12),
            validation_end=date(2025, 4, 10),   # 30 days
        )
        self.assertEqual(s.train_days, 70)

    def test_validation_days_property_correct(self):
        """WalkForwardSplit.validation_days returns inclusive day count."""
        s = WalkForwardSplit(
            fold_index=0,
            train_start=date(2025, 1, 1),
            train_end=date(2025, 3, 11),
            validation_start=date(2025, 3, 12),
            validation_end=date(2025, 4, 10),
        )
        self.assertEqual(s.validation_days, 30)


class TestNoForbiddenImports(unittest.TestCase):

    def test_no_subprocess_in_module(self):
        import mqk_research.scanner.walkforward as mod
        self.assertFalse(hasattr(mod, "subprocess"), "subprocess must not be imported")

    def test_no_network_imports(self):
        import mqk_research.scanner.walkforward as mod
        source = __import__("pathlib").Path(mod.__file__).read_text(encoding="utf-8")
        for forbidden in ["import requests", "import urllib", "import aiohttp",
                          "import http.client", "import psycopg", "import sqlalchemy"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden import: {forbidden!r}")

    def test_no_broker_oms_references(self):
        import mqk_research.scanner.walkforward as mod
        source = __import__("pathlib").Path(mod.__file__).read_text(encoding="utf-8")
        for forbidden in ["/v2/orders", "submit_order", "BrokerGateway", "oms_outbox"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden reference: {forbidden!r}")


if __name__ == "__main__":
    unittest.main()
