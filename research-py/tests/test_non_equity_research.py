"""
Batch 9 regression tests — non-equity research truth.

Covers four patches:

RESEARCH-OPTIONS-01
  - options_stub.load_options_chain_pg raises NotImplementedError (not RuntimeError, not silent)
  - OptionsChainQuery is still a valid frozen dataclass (contract shape preserved)

RESEARCH-FUTURES-01
  - futures_stub.load_futures_history_pg raises NotImplementedError (not RuntimeError, not silent)
  - FuturesHistoryQuery is still a valid frozen dataclass (contract shape preserved)

RESEARCH-PHASE2-01
  - CLI raises RuntimeError for OPTIONS asset_class policy before touching the DB
  - CLI raises RuntimeError for FUTURES asset_class policy before touching the DB
  - The error message names the unsupported asset class and does not imply success
  - No run directory is created (no phantom artifact written)

RESEARCH-CORP-ACTIONS-01
  - _earnings_flags_optional returns None when corporate_events table is absent
  - _earnings_flags_optional returns None when schema columns are missing
  - _earnings_flags_optional returns None when query raises
  - _earnings_flags_optional returns a real DataFrame (not None) when query succeeds
    with zero rows — genuinely checked, no events found is NOT the same as unavailable
  - build_universe_swing_v1 sets stubbed_earnings=True when earnings_flags is None
  - build_universe_swing_v1 sets stubbed_earnings=False when real flags are provided
  - stubbed_earnings=True propagates correctly: universe builder returns it
    (regression for the prior bug where _earnings_flags_optional always returned a
     DataFrame so stubbed_earnings was always False in the manifest)

All tests are pure in-process. No DB, no network, no file I/O (except RESEARCH-PHASE2-01
which uses a temp dir for the policy file and verifies no run dir is created).
"""
from __future__ import annotations

import tempfile
import unittest
from datetime import timezone
from pathlib import Path
from unittest.mock import MagicMock, patch

import pandas as pd

from mqk_research.data.adapters.options_stub import OptionsChainQuery, load_options_chain_pg
from mqk_research.data.adapters.futures_stub import FuturesHistoryQuery, load_futures_history_pg
from mqk_research.universe.build import build_universe_swing_v1


# ---------------------------------------------------------------------------
# RESEARCH-OPTIONS-01
# ---------------------------------------------------------------------------

class TestOptionsAdapterNotSupported(unittest.TestCase):
    def test_load_options_chain_pg_raises_not_implemented(self):
        """Adapter must raise NotImplementedError, not silently succeed or return data."""
        q = OptionsChainQuery(
            symbol="AAPL",
            asof_utc=pd.Timestamp("2026-01-15T00:00:00Z"),
        )
        with self.assertRaises(NotImplementedError) as ctx:
            load_options_chain_pg(query=q)
        msg = str(ctx.exception)
        self.assertIn("not supported", msg.lower())
        self.assertIn("AAPL", msg)

    def test_options_chain_query_is_frozen_dataclass(self):
        """Contract shape: OptionsChainQuery must remain frozen and constructable."""
        q = OptionsChainQuery(
            symbol="SPY",
            asof_utc=pd.Timestamp("2026-03-01T00:00:00Z"),
            expiry_utc=pd.Timestamp("2026-04-18T00:00:00Z"),
        )
        self.assertEqual(q.symbol, "SPY")
        self.assertIsNotNone(q.expiry_utc)
        # Frozen: assignment must raise
        with self.assertRaises((AttributeError, TypeError)):
            q.symbol = "QQQ"  # type: ignore[misc]

    def test_options_adapter_error_mentions_schema_prerequisite(self):
        """Error message must mention the missing infrastructure, not just 'stub'."""
        q = OptionsChainQuery(symbol="TSLA", asof_utc=pd.Timestamp("2026-01-15T00:00:00Z"))
        with self.assertRaises(NotImplementedError) as ctx:
            load_options_chain_pg(query=q)
        msg = str(ctx.exception)
        # Must mention what is required, not just say "stub" (stub theater was the old behavior)
        self.assertTrue(
            "schema" in msg.lower() or "pipeline" in msg.lower() or "infrastructure" in msg.lower(),
            f"Error message should name missing prerequisites: {msg!r}",
        )


# ---------------------------------------------------------------------------
# RESEARCH-FUTURES-01
# ---------------------------------------------------------------------------

class TestFuturesAdapterNotSupported(unittest.TestCase):
    def test_load_futures_history_pg_raises_not_implemented(self):
        """Adapter must raise NotImplementedError, not silently succeed or return data."""
        q = FuturesHistoryQuery(
            root="ES",
            asof_utc=pd.Timestamp("2026-01-15T00:00:00Z"),
            start_utc=pd.Timestamp("2025-07-01T00:00:00Z"),
            end_utc=pd.Timestamp("2026-01-15T00:00:00Z"),
        )
        with self.assertRaises(NotImplementedError) as ctx:
            load_futures_history_pg(query=q)
        msg = str(ctx.exception)
        self.assertIn("not supported", msg.lower())
        self.assertIn("ES", msg)

    def test_futures_history_query_is_frozen_dataclass(self):
        """Contract shape: FuturesHistoryQuery must remain frozen and constructable."""
        q = FuturesHistoryQuery(
            root="NQ",
            asof_utc=pd.Timestamp("2026-03-01T00:00:00Z"),
            start_utc=pd.Timestamp("2025-09-01T00:00:00Z"),
            end_utc=pd.Timestamp("2026-03-01T00:00:00Z"),
            contract="NQM2026",
            roll_rule="front_month",
        )
        self.assertEqual(q.root, "NQ")
        self.assertEqual(q.contract, "NQM2026")
        with self.assertRaises((AttributeError, TypeError)):
            q.root = "ES"  # type: ignore[misc]

    def test_futures_adapter_error_mentions_schema_prerequisite(self):
        """Error message must name what is missing, not just say 'stub'."""
        q = FuturesHistoryQuery(
            root="CL",
            asof_utc=pd.Timestamp("2026-01-15T00:00:00Z"),
            start_utc=pd.Timestamp("2025-07-01T00:00:00Z"),
            end_utc=pd.Timestamp("2026-01-15T00:00:00Z"),
        )
        with self.assertRaises(NotImplementedError) as ctx:
            load_futures_history_pg(query=q)
        msg = str(ctx.exception)
        self.assertTrue(
            "schema" in msg.lower() or "pipeline" in msg.lower() or "infrastructure" in msg.lower(),
            f"Error message should name missing prerequisites: {msg!r}",
        )


# ---------------------------------------------------------------------------
# RESEARCH-PHASE2-01
# ---------------------------------------------------------------------------

def _write_policy_yaml(tmpdir: Path, asset_class: str, policy_name: str) -> Path:
    content = (
        f"policy_name: {policy_name}\n"
        f"asset_class: {asset_class}\n"
        f"schema_version: '1'\n"
    )
    p = tmpdir / f"policy_{asset_class.lower()}.yaml"
    p.write_text(content, encoding="utf-8")
    return p


class TestPhase2CLIRefusal(unittest.TestCase):
    def setUp(self):
        self._tmpdir = tempfile.mkdtemp()
        self.tmpdir = Path(self._tmpdir)

    def tearDown(self):
        import shutil
        shutil.rmtree(self._tmpdir, ignore_errors=True)

    def _run_cli(self, policy_path: Path, asset_class: str) -> None:
        from mqk_research.cli import main
        out_dir = self.tmpdir / "runs"
        main([
            "run",
            "--policy", str(policy_path),
            "--asof-utc", "2026-01-15T00:00:00Z",
            "--symbols", "AAPL,MSFT",
            "--out", str(out_dir),
        ])

    def test_options_policy_raises_runtime_error(self):
        """CLI must raise RuntimeError for OPTIONS asset class, not write any artifact."""
        policy = _write_policy_yaml(self.tmpdir, "OPTIONS", "options_test")
        with self.assertRaises(RuntimeError) as ctx:
            self._run_cli(policy, "OPTIONS")
        msg = str(ctx.exception)
        self.assertIn("OPTIONS", msg)
        self.assertIn("not supported", msg.lower())

    def test_futures_policy_raises_runtime_error(self):
        """CLI must raise RuntimeError for FUTURES asset class, not write any artifact."""
        policy = _write_policy_yaml(self.tmpdir, "FUTURES", "futures_test")
        with self.assertRaises(RuntimeError) as ctx:
            self._run_cli(policy, "FUTURES")
        msg = str(ctx.exception)
        self.assertIn("FUTURES", msg)
        self.assertIn("not supported", msg.lower())

    def test_options_refusal_creates_no_run_directory(self):
        """OPTIONS refusal must not write any artifact or run directory."""
        policy = _write_policy_yaml(self.tmpdir, "OPTIONS", "options_no_artifact")
        out_dir = self.tmpdir / "runs"
        from mqk_research.cli import main
        with self.assertRaises(RuntimeError):
            main([
                "run",
                "--policy", str(policy),
                "--asof-utc", "2026-01-15T00:00:00Z",
                "--symbols", "AAPL",
                "--out", str(out_dir),
            ])
        # No run directory should exist — no phantom artifact written.
        self.assertFalse(
            out_dir.exists(),
            "OPTIONS refusal must not create any output directory or artifact",
        )

    def test_futures_refusal_creates_no_run_directory(self):
        """FUTURES refusal must not write any artifact or run directory."""
        policy = _write_policy_yaml(self.tmpdir, "FUTURES", "futures_no_artifact")
        out_dir = self.tmpdir / "runs"
        from mqk_research.cli import main
        with self.assertRaises(RuntimeError):
            main([
                "run",
                "--policy", str(policy),
                "--asof-utc", "2026-01-15T00:00:00Z",
                "--symbols", "ES",
                "--out", str(out_dir),
            ])
        self.assertFalse(
            out_dir.exists(),
            "FUTURES refusal must not create any output directory or artifact",
        )

    def test_error_message_names_equity_as_supported_path(self):
        """The refusal error should tell the operator what IS supported."""
        policy = _write_policy_yaml(self.tmpdir, "OPTIONS", "options_guidance")
        from mqk_research.cli import main
        out_dir = self.tmpdir / "runs"
        with self.assertRaises(RuntimeError) as ctx:
            main([
                "run",
                "--policy", str(policy),
                "--asof-utc", "2026-01-15T00:00:00Z",
                "--symbols", "AAPL",
                "--out", str(out_dir),
            ])
        msg = str(ctx.exception)
        self.assertIn("EQUITY", msg, "Error should name EQUITY as the supported asset class")


# ---------------------------------------------------------------------------
# RESEARCH-CORP-ACTIONS-01
# ---------------------------------------------------------------------------

def _make_mock_engine_no_table() -> MagicMock:
    """Engine where to_regclass returns None (table absent)."""
    cxn = MagicMock()
    cxn.execute.return_value.scalar.return_value = None
    engine = MagicMock()
    engine.connect.return_value.__enter__ = MagicMock(return_value=cxn)
    engine.connect.return_value.__exit__ = MagicMock(return_value=False)
    return engine


def _make_mock_engine_bad_schema() -> MagicMock:
    """Engine where to_regclass returns non-None but columns are missing required ones."""
    cxn = MagicMock()
    # First call (to_regclass) returns table present
    # Second call (information_schema.columns) returns columns without symbol/event_type/ts
    scalars = iter(["public.corporate_events"])
    cxn.execute.return_value.scalar.side_effect = lambda: next(scalars)
    # fetchall returns columns that don't include 'symbol', 'event_type', or any ts candidate
    cxn.execute.return_value.fetchall.return_value = [
        ("id", "bigint"),
        ("notes", "text"),
    ]
    engine = MagicMock()
    engine.connect.return_value.__enter__ = MagicMock(return_value=cxn)
    engine.connect.return_value.__exit__ = MagicMock(return_value=False)
    return engine


class TestEarningsFlagsOptional(unittest.TestCase):
    """
    Unit tests for _earnings_flags_optional.

    These tests prove the None-return contract: the function returns None when
    authoritative data is unavailable, and a real DataFrame only when the query
    actually ran against a schema-compatible table.
    """

    def _call(self, engine, symbols, asof_utc="2026-01-15T00:00:00Z", days_ahead=14):
        from mqk_research.cli import _earnings_flags_optional
        return _earnings_flags_optional(
            engine,
            symbols,
            pd.Timestamp(asof_utc),
            days_ahead=days_ahead,
        )

    def test_returns_none_when_symbols_empty(self):
        """No symbols → None (nothing to check, not a real empty result)."""
        engine = MagicMock()
        result = self._call(engine, [])
        self.assertIsNone(result)
        engine.connect.assert_not_called()

    def test_returns_none_when_table_absent(self):
        """corporate_events table absent → None (data unavailable, not 'no events')."""
        cxn = MagicMock()
        cxn.execute.return_value.scalar.return_value = None
        engine = MagicMock()
        engine.connect.return_value.__enter__ = MagicMock(return_value=cxn)
        engine.connect.return_value.__exit__ = MagicMock(return_value=False)

        result = self._call(engine, ["AAPL", "MSFT"])
        self.assertIsNone(result, "Table absent must return None, not a stub DataFrame")

    def test_returns_none_when_required_columns_missing(self):
        """Table present but missing required columns → None."""
        # Two separate connect() calls: first for to_regclass, second for columns.
        # We use a counter to distinguish them.
        call_count = [0]

        def make_cxn():
            cxn = MagicMock()
            call_count[0] += 1
            if call_count[0] == 1:
                # First connect: to_regclass returns non-None (table present)
                cxn.execute.return_value.scalar.return_value = "public.corporate_events"
                cxn.execute.return_value.fetchall.return_value = [
                    ("id", "bigint"),
                    ("notes", "text"),
                ]
            return cxn

        engine = MagicMock()
        engine.connect.return_value.__enter__ = MagicMock(side_effect=make_cxn)
        engine.connect.return_value.__exit__ = MagicMock(return_value=False)

        result = self._call(engine, ["AAPL"])
        self.assertIsNone(result, "Missing required columns must return None")

    def test_returns_none_when_query_raises(self):
        """Query execution error → None (fail closed, not silent false)."""
        call_count = [0]

        def make_cxn():
            cxn = MagicMock()
            call_count[0] += 1
            if call_count[0] == 1:
                # First connect block: to_regclass + columns
                cxn.execute.return_value.scalar.return_value = "public.corporate_events"
                cxn.execute.return_value.fetchall.return_value = [
                    ("symbol", "character varying"),
                    ("event_type", "character varying"),
                    ("event_ts_utc", "timestamp with time zone"),
                ]
            else:
                # Second connect block: pd.read_sql raises
                cxn.execute.side_effect = Exception("DB connection lost")
            return cxn

        engine = MagicMock()
        engine.connect.return_value.__enter__ = MagicMock(side_effect=make_cxn)
        engine.connect.return_value.__exit__ = MagicMock(return_value=False)

        with patch("mqk_research.cli.pd.read_sql", side_effect=Exception("query failed")):
            result = self._call(engine, ["AAPL"])
        self.assertIsNone(result, "Query failure must return None, not silent False")

    def test_returns_real_dataframe_when_query_succeeds_no_events(self):
        """
        Table present, schema valid, query returns empty → real DataFrame with
        earnings_within_14d=False. This is NOT stubbed — earnings were genuinely checked.
        """
        syms = ["AAPL", "MSFT"]
        call_count = [0]

        def make_cxn():
            cxn = MagicMock()
            call_count[0] += 1
            if call_count[0] == 1:
                cxn.execute.return_value.scalar.return_value = "public.corporate_events"
                cxn.execute.return_value.fetchall.return_value = [
                    ("symbol", "character varying"),
                    ("event_type", "character varying"),
                    ("event_ts_utc", "timestamp with time zone"),
                ]
            return cxn

        engine = MagicMock()
        engine.connect.return_value.__enter__ = MagicMock(side_effect=make_cxn)
        engine.connect.return_value.__exit__ = MagicMock(return_value=False)

        empty_df = pd.DataFrame(columns=["symbol", "ts_utc", "event_type"])
        with patch("mqk_research.cli.pd.read_sql", return_value=empty_df):
            result = self._call(engine, syms)

        self.assertIsNotNone(result, "Successful empty query must return a real DataFrame (not None)")
        self.assertIsInstance(result, pd.DataFrame)
        self.assertIn("symbol", result.columns)
        self.assertIn("earnings_within_14d", result.columns)
        self.assertEqual(sorted(result["symbol"].tolist()), sorted(syms))
        self.assertTrue((result["earnings_within_14d"] == False).all())  # noqa: E712

    def test_returns_dataframe_with_flagged_symbols_when_events_found(self):
        """When events found, the symbols with upcoming earnings are flagged True."""
        syms = ["AAPL", "MSFT", "GOOG"]
        call_count = [0]

        def make_cxn():
            cxn = MagicMock()
            call_count[0] += 1
            if call_count[0] == 1:
                cxn.execute.return_value.scalar.return_value = "public.corporate_events"
                cxn.execute.return_value.fetchall.return_value = [
                    ("symbol", "character varying"),
                    ("event_type", "character varying"),
                    ("event_ts_utc", "timestamp with time zone"),
                ]
            return cxn

        engine = MagicMock()
        engine.connect.return_value.__enter__ = MagicMock(side_effect=make_cxn)
        engine.connect.return_value.__exit__ = MagicMock(return_value=False)

        events_df = pd.DataFrame({
            "symbol": ["AAPL", "GOOG"],
            "ts_utc": [pd.Timestamp("2026-01-20T00:00:00Z"), pd.Timestamp("2026-01-22T00:00:00Z")],
            "event_type": ["EARNINGS", "EARNINGS"],
        })
        with patch("mqk_research.cli.pd.read_sql", return_value=events_df):
            result = self._call(engine, syms)

        self.assertIsNotNone(result)
        flags = dict(zip(result["symbol"].tolist(), result["earnings_within_14d"].tolist()))
        self.assertTrue(flags["AAPL"])
        self.assertFalse(flags["MSFT"])
        self.assertTrue(flags["GOOG"])


# ---------------------------------------------------------------------------
# RESEARCH-CORP-ACTIONS-01 — stubbed_earnings propagation via universe builder
# ---------------------------------------------------------------------------

def _make_minimal_features(symbols: list[str]) -> pd.DataFrame:
    """Minimal features DataFrame satisfying build_universe_swing_v1 requirements."""
    rows = []
    for sym in symbols:
        rows.append({
            "symbol": sym,
            "ts_utc": pd.Timestamp("2026-01-15T00:00:00Z"),
            "close": 100.0,
            "adv_usd_20": 5_000_000.0,
            "atr_pct_20": 0.02,
            "ret_60d": 0.05,
            "trend_proxy": 0.03,
        })
    return pd.DataFrame(rows)


_SWING_POLICY = {
    "policy_name": "test_swing",
    "asset_class": "EQUITY",
    "bars": {"timeframe": "1D", "lookback_days": 140},
    "filters": {"min_price": 5.0, "min_adv_usd_20": 1_000_000.0},
    "rank": {"top_k": 50, "formula": "ret_60d + trend_proxy"},
}


class TestUniverseBuilderStubbedEarnings(unittest.TestCase):
    def test_stubbed_earnings_true_when_earnings_flags_none(self):
        """
        build_universe_swing_v1 with earnings_flags=None must return stubbed_earnings=True.
        This is the regression case: previously _earnings_flags_optional always returned a
        DataFrame, so stubbed_earnings was never set True even when no real data was used.
        """
        feats = _make_minimal_features(["AAPL", "MSFT"])
        result = build_universe_swing_v1(features=feats, policy=_SWING_POLICY, earnings_flags=None)
        self.assertTrue(
            result.stubbed_earnings,
            "stubbed_earnings must be True when earnings_flags=None (data unavailable)",
        )

    def test_stubbed_earnings_false_when_real_flags_provided(self):
        """When real earnings flags are provided, stubbed_earnings must be False."""
        feats = _make_minimal_features(["AAPL", "MSFT"])
        real_flags = pd.DataFrame({
            "symbol": ["AAPL", "MSFT"],
            "earnings_within_14d": [False, False],
        })
        result = build_universe_swing_v1(features=feats, policy=_SWING_POLICY, earnings_flags=real_flags)
        self.assertFalse(
            result.stubbed_earnings,
            "stubbed_earnings must be False when real earnings flags are provided",
        )

    def test_earnings_within_14d_set_false_when_flags_none(self):
        """When earnings_flags=None, universe builder sets all earnings_within_14d=False."""
        feats = _make_minimal_features(["AAPL"])
        result = build_universe_swing_v1(features=feats, policy=_SWING_POLICY, earnings_flags=None)
        self.assertIn("earnings_within_14d", result.df.columns)
        self.assertTrue((result.df["earnings_within_14d"] == False).all())  # noqa: E712

    def test_symbol_with_earnings_excluded_from_universe(self):
        """Symbol flagged earnings_within_14d=True is excluded from the universe."""
        feats = _make_minimal_features(["AAPL", "MSFT"])
        flags = pd.DataFrame({
            "symbol": ["AAPL", "MSFT"],
            "earnings_within_14d": [True, False],  # AAPL has upcoming earnings
        })
        result = build_universe_swing_v1(features=feats, policy=_SWING_POLICY, earnings_flags=flags)
        self.assertFalse(result.stubbed_earnings)
        symbols_in_universe = result.df["symbol"].tolist()
        self.assertNotIn("AAPL", symbols_in_universe, "AAPL should be excluded due to earnings")
        self.assertIn("MSFT", symbols_in_universe)


if __name__ == "__main__":
    unittest.main()
