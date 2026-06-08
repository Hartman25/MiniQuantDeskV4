"""
SYMBOL-INPUTS-RUNNER-01 — Tests for the symbol_inputs operational artifact runner.

Covers symbol-list resolution (watchlist / explicit list / forged-live-lock),
local bars-file loading and normalization (JSON, scanner-style CSV,
backtest-style micros CSV), liquidity-sidecar handling, fail-closed behavior,
artifact output + premarket integration, and import-boundary safety.

All tests are pure in-process, deterministic-fixture based, using tempfile
directories for local file I/O. No DB, no broker, no network, no daemon,
no subprocess.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m pytest research-py/tests/test_scanner_symbol_inputs_runner.py -q
"""
from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from mqk_research.scanner.data_quality import REASON_NO_BARS
from mqk_research.scanner.liquidity import LiquidityMetrics
from mqk_research.scanner.symbol_inputs import (
    REASON_LIQUIDITY_METRICS_MISSING,
    SCHEMA_VERSION,
    extract_symbol_inputs_map,
    load_symbol_inputs_artifact,
)
from mqk_research.scanner.premarket_revalidation import (
    PremarketRevalidationConfig,
    evaluate_premarket_watchlist,
)
from mqk_research.scanner.symbol_inputs_runner import (
    REASON_BARS_ROOT_MISSING,
    REASON_LIQUIDITY_SIDECAR_MALFORMED,
    REASON_MALFORMED_BARS_FILE,
    REASON_NO_INPUT_SOURCE,
    REASON_NO_SYMBOLS_RESOLVED,
    REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN,
    REASON_WATCHLIST_LOAD_FAILED,
    SymbolInputsRunnerConfig,
    SymbolInputsRunnerResult,
    load_liquidity_metrics_for_symbol,
    load_symbols_from_watchlist,
    normalize_bars_csv,
    normalize_bars_json,
    resolve_bars_path,
    run_symbol_inputs_producer,
)


# ---------------------------------------------------------------------------
# Bar fixtures — deterministic, mirrors the proven pattern in
# test_scanner_symbol_inputs.py (30 bars: 10 flat @ 99.0, 20 rising to 100.0
# -> classifies as trending_up; passes data-quality + regime).
# ---------------------------------------------------------------------------

_NOW = datetime(2026, 6, 5, 14, 0, 0, tzinfo=timezone.utc)


def _bar(ts: datetime, close: float, *, spread: float = 0.8, volume: int = 2_000_000) -> dict[str, Any]:
    return {
        "ts": ts.isoformat(),
        "open": close,
        "high": close + spread / 2.0,
        "low": close - spread / 2.0,
        "close": close,
        "volume": volume,
    }


def _trending_up_bars(
    *,
    flat_n: int = 10,
    trend_n: int = 20,
    flat_close: float = 99.0,
    trend_end_close: float = 100.0,
    spread: float = 0.8,
    interval_minutes: float = 5.0,
    latest_ts: datetime | None = None,
) -> list[dict[str, Any]]:
    if latest_ts is None:
        latest_ts = _NOW - timedelta(minutes=interval_minutes)
    total = flat_n + trend_n
    closes = [flat_close] * flat_n + [
        flat_close + (trend_end_close - flat_close) * (i / max(trend_n - 1, 1))
        for i in range(trend_n)
    ]
    bars = []
    for idx, c in enumerate(closes):
        ts = latest_ts - timedelta(minutes=(total - 1 - idx) * interval_minutes)
        bars.append(_bar(ts, c, spread=spread))
    return bars


_PASSING_LIQUIDITY_METRICS = LiquidityMetrics(
    latest_close=190.0,
    avg_daily_volume_shares=50_000_000.0,
    avg_daily_dollar_volume=9_500_000_000.0,
    relative_volume=1.5,
    spread_estimate_bps=8.0,
    slippage_estimate_bps=6.0,
    round_trip_cost_bps=14.0,
)

_PASSING_LIQUIDITY_SIDECAR: dict[str, float] = {
    "latest_close": 190.0,
    "avg_daily_volume_shares": 50_000_000.0,
    "avg_daily_dollar_volume": 9_500_000_000.0,
    "relative_volume": 1.5,
    "spread_estimate_bps": 8.0,
    "slippage_estimate_bps": 6.0,
    "round_trip_cost_bps": 14.0,
}


def _make_watchlist(symbols: list[str], *, approved_for_live: bool = False) -> dict[str, Any]:
    return {
        "schema_version": "watchlist-v1",
        "mode": "paper",
        "approved_for_live": approved_for_live,
        "max_symbols_to_trade": len(symbols),
        "max_concurrent_positions": len(symbols),
        "symbols": list(symbols),
        "strategy_assignments": {s: "intraday_scalper" for s in symbols},
    }


def _write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _scanner_csv_text(bars: list[dict[str, Any]]) -> str:
    lines = ["ts,open,high,low,close,volume,is_complete"]
    for bar in bars:
        lines.append(
            f"{bar['ts']},{bar['open']},{bar['high']},{bar['low']},"
            f"{bar['close']},{bar['volume']},1"
        )
    return "\n".join(lines) + "\n"


def _backtest_csv_text(symbol: str, bars: list[dict[str, Any]]) -> str:
    lines = ["symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete"]
    for bar in bars:
        end_ts = int(datetime.fromisoformat(bar["ts"]).timestamp())
        lines.append(
            f"{symbol},{end_ts},{int(round(bar['open'] * 1_000_000))},"
            f"{int(round(bar['high'] * 1_000_000))},{int(round(bar['low'] * 1_000_000))},"
            f"{int(round(bar['close'] * 1_000_000))},{bar['volume']},1"
        )
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Group A — symbol-list resolution / blocking
# ---------------------------------------------------------------------------

class TestLoadsSymbolsFromWatchlist(unittest.TestCase):
    def test_runner_resolves_symbols_from_watchlist_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            watchlist_path = root / "watchlist.json"
            _write_json(watchlist_path, _make_watchlist(["AAPL", "MSFT"]))
            bars_root = root / "bars"
            for sym in ("AAPL", "MSFT"):
                _write_json(bars_root / f"{sym}_5m.json", _trending_up_bars())

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out" / "symbol_inputs.json"),
                bars_root=str(bars_root),
                watchlist_path=str(watchlist_path),
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.symbols_requested, ["AAPL", "MSFT"])
            self.assertEqual(result.status, "complete")
            self.assertFalse(result.watchlist_live_approval_forbidden)
            self.assertFalse(result.approved_for_live)


class TestLoadsSymbolsFromExplicitList(unittest.TestCase):
    def test_runner_resolves_symbols_from_explicit_symbol_list(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out" / "symbol_inputs.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.symbols_requested, ["AAPL"])
            self.assertEqual(result.symbols_written, ["AAPL"])
            self.assertEqual(result.status, "complete")


class TestForgedLiveApprovalDoesNotPropagate(unittest.TestCase):
    def test_forged_approved_for_live_in_watchlist_is_reported_not_propagated(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            watchlist_path = root / "watchlist.json"
            _write_json(watchlist_path, _make_watchlist(["AAPL"], approved_for_live=True))
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())
            output_path = root / "out" / "symbol_inputs.json"

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(output_path),
                bars_root=str(bars_root),
                watchlist_path=str(watchlist_path),
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)

            self.assertTrue(result.watchlist_live_approval_forbidden)
            self.assertIn(REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN, result.failure_reasons)
            self.assertFalse(result.approved_for_live)

            written = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertFalse(written["approved_for_live"])

    def test_load_symbols_from_watchlist_reports_forged_live_flag(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "watchlist.json"
            _write_json(path, _make_watchlist(["AAPL"], approved_for_live=True))
            symbols, forged, error = load_symbols_from_watchlist(str(path))
            self.assertEqual(symbols, ["AAPL"])
            self.assertTrue(forged)
            self.assertIsNone(error)


class TestMissingInputSourceBlocks(unittest.TestCase):
    def test_neither_watchlist_nor_symbols_blocks(self) -> None:
        config = SymbolInputsRunnerConfig(
            trade_date="2026-06-08",
            output_path="unused.json",
            bars_root="unused",
        )
        result = run_symbol_inputs_producer(config)

        self.assertEqual(result.status, "blocked")
        self.assertIn(REASON_NO_INPUT_SOURCE, result.failure_reasons)
        self.assertIsNone(result.output_path)
        self.assertEqual(result.symbols_written, [])

    def test_unreadable_watchlist_blocks_with_explicit_reason(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            missing_path = Path(tmp) / "does_not_exist.json"
            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(Path(tmp) / "out.json"),
                bars_root=str(tmp),
                watchlist_path=str(missing_path),
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.status, "blocked")
            self.assertIn(REASON_WATCHLIST_LOAD_FAILED, result.failure_reasons)

    def test_watchlist_with_empty_symbols_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            watchlist_path = root / "watchlist.json"
            _write_json(watchlist_path, _make_watchlist([]))
            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(root),
                watchlist_path=str(watchlist_path),
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.status, "blocked")
            self.assertIn(REASON_NO_SYMBOLS_RESOLVED, result.failure_reasons)


class TestMissingBarsRootBlocks(unittest.TestCase):
    def test_missing_bars_root_directory_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(root / "does_not_exist"),
                symbols=["AAPL"],
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.status, "blocked")
            self.assertIn(REASON_BARS_ROOT_MISSING, result.failure_reasons)
            self.assertEqual(result.symbols_requested, ["AAPL"])
            self.assertIsNone(result.output_path)


class TestMissingBarsForSymbolFailsClosedAndPartial(unittest.TestCase):
    def test_symbol_without_bars_file_appears_with_fail_closed_record(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())
            # MSFT intentionally has no bars file.

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out" / "symbol_inputs.json"),
                bars_root=str(bars_root),
                symbols=["AAPL", "MSFT"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.status, "partial")
            self.assertEqual(result.missing_bars, ["MSFT"])
            self.assertIn("AAPL", result.symbols_written)
            self.assertIn("MSFT", result.symbols_written)

            written = load_symbol_inputs_artifact(result.output_path)
            self.assertFalse(written["symbols"]["MSFT"]["data_quality_passed"])
            self.assertEqual(written["symbols"]["MSFT"]["rejection_reason"], REASON_NO_BARS)


# ---------------------------------------------------------------------------
# Group B — bars loading / normalization
# ---------------------------------------------------------------------------

class TestScannerStyleJsonBarsLoadCorrectly(unittest.TestCase):
    def test_json_bars_pass_through_pipeline(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)

            self.assertTrue(written["symbols"]["AAPL"]["data_quality_passed"])
            self.assertEqual(written["symbols"]["AAPL"]["latest_close"], 100.0)

    def test_normalize_bars_json_accepts_dict_with_bars_key(self) -> None:
        bars = _trending_up_bars()
        normalized, reason = normalize_bars_json(json.dumps({"bars": bars}))
        self.assertIsNone(reason)
        self.assertEqual(len(normalized), len(bars))
        self.assertEqual(normalized[-1]["close"], bars[-1]["close"])

    def test_normalize_bars_json_fails_closed_on_non_list(self) -> None:
        normalized, reason = normalize_bars_json(json.dumps({"not_bars": []}))
        self.assertIsNone(normalized)
        self.assertEqual(reason, REASON_MALFORMED_BARS_FILE)


class TestScannerStyleCsvBarsLoadCorrectly(unittest.TestCase):
    def test_normalize_scanner_csv_parses_expected_fields(self) -> None:
        text = (
            "ts,open,high,low,close,volume,is_complete\n"
            "2026-06-05T13:30:00+00:00,99.0,99.4,98.6,99.0,2000000,1\n"
            "2026-06-05T13:35:00+00:00,99.0,99.5,98.7,99.2,2100000,0\n"
        )
        bars, reason = normalize_bars_csv(text)

        self.assertIsNone(reason)
        self.assertEqual(len(bars), 2)
        first = bars[0]
        self.assertEqual(first["ts"], "2026-06-05T13:30:00+00:00")
        self.assertEqual(first["open"], 99.0)
        self.assertEqual(first["high"], 99.4)
        self.assertEqual(first["low"], 98.6)
        self.assertEqual(first["close"], 99.0)
        self.assertEqual(first["volume"], 2_000_000.0)
        self.assertIs(first["is_complete"], True)
        self.assertIs(bars[1]["is_complete"], False)


class TestBacktestStyleCsvMicrosConvertCorrectly(unittest.TestCase):
    def test_normalize_backtest_csv_converts_micros_to_decimal_dollars(self) -> None:
        text = (
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n"
            "AAPL,1749130200,99000000,99400000,98600000,99200000,2000000,1\n"
        )
        bars, reason = normalize_bars_csv(text)

        self.assertIsNone(reason)
        self.assertEqual(len(bars), 1)
        bar = bars[0]
        self.assertEqual(bar["open"], 99.0)
        self.assertEqual(bar["high"], 99.4)
        self.assertEqual(bar["low"], 98.6)
        self.assertEqual(bar["close"], 99.2)
        self.assertEqual(bar["volume"], 2_000_000.0)
        self.assertIs(bar["is_complete"], True)
        expected_ts = datetime.fromtimestamp(1749130200, tz=timezone.utc).isoformat()
        self.assertEqual(bar["ts"], expected_ts)

    def test_runner_loads_backtest_style_csv_end_to_end(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            bars = _trending_up_bars()
            _write_text(bars_root / "AAPL_5m.csv", _backtest_csv_text("AAPL", bars))

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)

            self.assertEqual(result.status, "complete")
            self.assertTrue(written["symbols"]["AAPL"]["data_quality_passed"])
            self.assertAlmostEqual(written["symbols"]["AAPL"]["latest_close"], 100.0, places=6)


class TestMalformedBarsFileFailsClosed(unittest.TestCase):
    def test_malformed_json_bars_file_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_text(bars_root / "AAPL_5m.json", "{not valid json")

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)

            self.assertEqual(result.status, "partial")
            self.assertEqual(result.failed_symbols, ["AAPL"])
            self.assertIn(f"{REASON_MALFORMED_BARS_FILE}:AAPL", result.failure_reasons)

            written = load_symbol_inputs_artifact(result.output_path)
            self.assertFalse(written["symbols"]["AAPL"]["data_quality_passed"])
            self.assertEqual(written["symbols"]["AAPL"]["rejection_reason"], REASON_NO_BARS)

    def test_csv_with_unknown_header_fails_closed(self) -> None:
        bars, reason = normalize_bars_csv("foo,bar,baz\n1,2,3\n")
        self.assertIsNone(bars)
        self.assertEqual(reason, REASON_MALFORMED_BARS_FILE)

    def test_csv_with_unparseable_row_fails_closed(self) -> None:
        text = (
            "ts,open,high,low,close,volume\n"
            "2026-06-05T13:30:00+00:00,not_a_number,99.4,98.6,99.0,2000000\n"
        )
        bars, reason = normalize_bars_csv(text)
        self.assertIsNone(bars)
        self.assertEqual(reason, REASON_MALFORMED_BARS_FILE)


class TestIncompleteTrailingBarPreservedOrSkipped(unittest.TestCase):
    def test_normalize_csv_preserves_is_complete_flag(self) -> None:
        text = (
            "ts,open,high,low,close,volume,is_complete\n"
            "2026-06-05T13:30:00+00:00,99.0,99.4,98.6,99.0,2000000,true\n"
            "2026-06-05T13:35:00+00:00,99.0,99.5,98.7,99.2,2100000,false\n"
        )
        bars, reason = normalize_bars_csv(text)
        self.assertIsNone(reason)
        self.assertIs(bars[0]["is_complete"], True)
        self.assertIs(bars[1]["is_complete"], False)

    def test_trailing_incomplete_bar_is_skipped_by_producer_end_to_end(self) -> None:
        complete_bars = _trending_up_bars()
        trailing_incomplete = dict(_bar(_NOW, 105.0))
        trailing_incomplete["is_complete"] = False
        bars_with_trailing = complete_bars + [trailing_incomplete]

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", bars_with_trailing)

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)
            record = written["symbols"]["AAPL"]

            self.assertTrue(record["data_quality_passed"])
            # The trailing is_complete=False bar (close=105.0) must be skipped —
            # latest_close reflects the last *completed* bar (close=100.0).
            self.assertEqual(record["latest_close"], 100.0)


class TestResolveBarsPath(unittest.TestCase):
    def test_resolve_prefers_symbol_timeframe_json_over_other_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write_json(root / "AAPL_5m.json", [])
            _write_json(root / "AAPL.json", [])
            _write_text(root / "AAPL_5m.csv", "ts,open,high,low,close,volume\n")

            resolved = resolve_bars_path(str(root), "AAPL", "5m")
            self.assertEqual(resolved, root / "AAPL_5m.json")

    def test_resolve_returns_none_when_no_candidate_exists(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(resolve_bars_path(tmp, "AAPL", "5m"))


# ---------------------------------------------------------------------------
# Group C — artifact output / premarket integration
# ---------------------------------------------------------------------------

class TestArtifactOutput(unittest.TestCase):
    def _run(self, tmp: str) -> tuple[SymbolInputsRunnerResult, dict[str, Any]]:
        root = Path(tmp)
        bars_root = root / "bars"
        liquidity_root = root / "liquidity"
        _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())
        _write_json(liquidity_root / "AAPL.json", _PASSING_LIQUIDITY_SIDECAR)

        config = SymbolInputsRunnerConfig(
            trade_date="2026-06-08",
            output_path=str(root / "out" / "symbol_inputs.json"),
            bars_root=str(bars_root),
            liquidity_root=str(liquidity_root),
            symbols=["AAPL"],
            source="symbol_inputs_runner_test",
            now_utc=_NOW,
        )
        result = run_symbol_inputs_producer(config)
        artifact = load_symbol_inputs_artifact(result.output_path)
        return result, artifact

    def test_runner_writes_symbol_inputs_v1_with_expected_keys(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result, artifact = self._run(tmp)

            self.assertEqual(result.status, "complete")
            self.assertEqual(artifact["schema_version"], SCHEMA_VERSION)
            self.assertEqual(artifact["trade_date"], "2026-06-08")
            self.assertEqual(artifact["source"], "symbol_inputs_runner_test")
            self.assertIn("AAPL", artifact["symbols"])
            self.assertFalse(artifact["approved_for_live"])

    def test_output_consumable_by_extract_symbol_inputs_map(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            _, artifact = self._run(tmp)
            extracted = extract_symbol_inputs_map(artifact)

            self.assertIn("AAPL", extracted)
            self.assertTrue(extracted["AAPL"]["data_quality_passed"])
            self.assertTrue(extracted["AAPL"]["liquidity_passed"])

    def test_produced_artifact_feeds_premarket_revalidation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            _, artifact = self._run(tmp)
            extracted = extract_symbol_inputs_map(artifact)
            watchlist = _make_watchlist(["AAPL"])

            premarket_result = evaluate_premarket_watchlist(
                watchlist, extracted, PremarketRevalidationConfig(), reference_utc=_NOW
            )

            self.assertTrue(premarket_result.passed, premarket_result.failure_reasons)
            self.assertEqual(premarket_result.top_symbol, "AAPL")


# ---------------------------------------------------------------------------
# Group D — liquidity sidecar handling
# ---------------------------------------------------------------------------

class TestLiquidityHandling(unittest.TestCase):
    def test_absent_liquidity_metrics_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)
            record = written["symbols"]["AAPL"]

            self.assertFalse(record["liquidity_passed"])
            self.assertIn(REASON_LIQUIDITY_METRICS_MISSING, record["reason_tags"])

    def test_provided_liquidity_sidecar_passes_when_thresholds_met(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            liquidity_root = root / "liquidity"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())
            _write_json(liquidity_root / "AAPL.json", _PASSING_LIQUIDITY_SIDECAR)

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                liquidity_root=str(liquidity_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)
            record = written["symbols"]["AAPL"]

            self.assertTrue(record["liquidity_passed"])
            self.assertEqual(record["spread_estimate_bps"], 8.0)
            self.assertEqual(record["relative_volume"], 1.5)

    def test_bad_liquidity_sidecar_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bars_root = root / "bars"
            liquidity_root = root / "liquidity"
            _write_json(bars_root / "AAPL_5m.json", _trending_up_bars())
            _write_json(liquidity_root / "AAPL.json", {"latest_close": "not_a_number"})

            config = SymbolInputsRunnerConfig(
                trade_date="2026-06-08",
                output_path=str(root / "out.json"),
                bars_root=str(bars_root),
                liquidity_root=str(liquidity_root),
                symbols=["AAPL"],
                now_utc=_NOW,
            )
            result = run_symbol_inputs_producer(config)
            written = load_symbol_inputs_artifact(result.output_path)
            record = written["symbols"]["AAPL"]

            self.assertFalse(record["liquidity_passed"])
            self.assertIn(f"{REASON_LIQUIDITY_SIDECAR_MALFORMED}:AAPL", result.failure_reasons)
            self.assertIn(REASON_LIQUIDITY_METRICS_MISSING, record["reason_tags"])

    def test_load_liquidity_metrics_returns_none_when_absent(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            metrics, reason = load_liquidity_metrics_for_symbol(tmp, "AAPL")
            self.assertIsNone(metrics)
            self.assertIsNone(reason)

    def test_load_liquidity_metrics_parses_valid_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write_json(root / "AAPL.json", _PASSING_LIQUIDITY_SIDECAR)
            metrics, reason = load_liquidity_metrics_for_symbol(str(root), "AAPL")

            self.assertIsNone(reason)
            self.assertIsInstance(metrics, LiquidityMetrics)
            self.assertEqual(metrics.relative_volume, 1.5)


# ---------------------------------------------------------------------------
# Group E — safety / import-boundary guards
# ---------------------------------------------------------------------------

class TestNoBrokerOmsRuntimeDbNetworkImports(unittest.TestCase):
    def _module_source(self) -> str:
        from mqk_research.scanner import symbol_inputs_runner
        return Path(symbol_inputs_runner.__file__).read_text(encoding="utf-8")

    def test_no_forbidden_imports(self) -> None:
        source = self._module_source()
        for forbidden in (
            "import requests", "import urllib", "import http.client", "import aiohttp",
            "import psycopg", "import sqlalchemy",
            "import subprocess", "os.system",
            "from mqk_daemon", "import mqk_daemon",
            "from mqk_runtime", "import mqk_runtime",
            "BrokerGateway", "broker_adapter",
        ):
            self.assertNotIn(forbidden, source, f"forbidden import/reference present: {forbidden}")

    def test_no_order_or_signal_endpoint_references(self) -> None:
        source = self._module_source()
        for forbidden in (
            "/v2/orders", "submit_order", "place_order", "enqueue_outbox",
            "/api/v1/strategy/signal", "strategy/signal", "oms_outbox", "oms_inbox",
        ):
            self.assertNotIn(forbidden, source, f"forbidden order/signal reference present: {forbidden}")

    def test_no_live_or_eligible_for_live_true(self) -> None:
        source = self._module_source()
        self.assertNotIn('approved_for_live"] = True', source)
        self.assertNotIn("approved_for_live: bool = True", source)
        self.assertNotIn("eligible_for_live", source)


# ---------------------------------------------------------------------------
# Group F — runner result hard invariants
# ---------------------------------------------------------------------------

class TestRunnerResultHardInvariants(unittest.TestCase):
    def test_config_approved_for_live_always_false(self) -> None:
        config = SymbolInputsRunnerConfig(
            trade_date="2026-06-08", output_path="x.json", bars_root="bars", symbols=["AAPL"],
        )
        self.assertFalse(config.approved_for_live)

    def test_result_approved_for_live_always_false_even_when_blocked(self) -> None:
        config = SymbolInputsRunnerConfig(trade_date="2026-06-08", output_path="x.json")
        result = run_symbol_inputs_producer(config)
        self.assertFalse(result.approved_for_live)
        self.assertFalse(result.to_dict()["approved_for_live"])


if __name__ == "__main__":
    unittest.main()
