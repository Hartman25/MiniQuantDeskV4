"""
PAPER-READINESS-RUNNER-01 — tests for the operational paper-readiness pipeline
runner (`mqk_research.scanner.paper_readiness_runner`).

Covers:
  1-5   toggle behavior (live_handoff hard block, mode lock, paper_readiness/
        symbol_inputs/watchlist_promotion enable gates)
  6     the operational passing path — full chain, real temp-dir fixtures,
        every conventional artifact written and reloadable
  7-15  fail-closed paths (missing/malformed inputs at every chain link,
        upstream-runner blocked/partial propagation, operator-review-only gap)
  16-21 safety invariants (approved_for_live forced False at config/result/
        artifact layers, forged live approval never propagates, no forbidden
        imports/references, config loader cannot be used as a live side channel)

All fixtures are deterministic in-memory + temp-dir only. No daemon/broker/DB/
network/subprocess imports; no order submission; no strategy signal injection;
no live routing. This test does NOT prove market/live/paper truth — it proves
the runner composes the existing proven evaluators honestly and fails closed
on every missing/malformed/disabled/forged input.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.backtest_gates import BacktestGateConfig, apply_backtest_gates
from mqk_research.scanner import paper_readiness_runner as prr_module
from mqk_research.scanner.paper_readiness_runner import (
    SCHEMA_VERSION,
    STATUS_BLOCKED,
    STATUS_PARTIAL,
    STATUS_READY_FOR_OPERATOR_REVIEW,
    STATUS_READY_FOR_PAPER_HANDOFF,
    REASON_LIVE_HANDOFF_FORBIDDEN,
    REASON_MODE_NOT_PAPER,
    REASON_PAPER_READINESS_DISABLED,
    REASON_SYMBOL_INPUTS_DISABLED,
    REASON_WATCHLIST_PROMOTION_DISABLED,
    REASON_WATCHLIST_PATH_MISSING,
    REASON_WATCHLIST_LOAD_FAILED,
    REASON_NO_RANKED_CANDIDATES,
    REASON_BARS_ROOT_MISSING,
    REASON_STRATEGY_FIT_DIR_MISSING,
    REASON_STRATEGY_FIT_MISSING,
    REASON_SYMBOL_INPUTS_BLOCKED,
    REASON_SYMBOL_INPUTS_PARTIAL,
    REASON_FORGED_LIVE_APPROVAL_FORBIDDEN,
    REASON_CONFIG_LOAD_FAILED,
    PaperReadinessConfig,
    PaperReadinessResult,
    run_paper_readiness_pipeline,
    load_config_from_json,
)
from mqk_research.scanner.watchlist_promotion import REASON_OPERATOR_REVIEW_REQUIRED


# ---------------------------------------------------------------------------
# Deterministic fixtures — symbols, time, bars, liquidity, strategy-fit
# ---------------------------------------------------------------------------

_NOW = datetime(2026, 6, 8, 14, 0, 0, tzinfo=timezone.utc)
_TRADE_DATE = "2026-06-08"
_SYMBOL = "AAPL"
_STRATEGY_ID = "intraday_scalper"
_REGIME = "trending_up"
_FRESH_LATEST_TS = _NOW - timedelta(minutes=5)


def _write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, default=str), encoding="utf-8")


def _bar(ts: datetime, close: float, *, spread: float = 0.8, volume: int = 2_000_000) -> dict[str, Any]:
    return {
        "ts": ts.isoformat(),
        "open": close,
        "high": close + spread / 2.0,
        "low": close - spread / 2.0,
        "close": close,
        "volume": volume,
    }


def _trending_up_bars(*, latest_ts: Optional[datetime] = None) -> list[dict[str, Any]]:
    """30 bars: 10 flat at 99.0, then 20 rising linearly to 100.0 -> trending_up.

    Mirrors the proven fixture from the watchlist-promotion end-to-end chain
    test that classifies as trending_up and passes data-quality + regime gates.
    """
    flat_n, trend_n = 10, 20
    flat_close, trend_end_close = 99.0, 100.0
    interval_minutes = 5.0
    if latest_ts is None:
        latest_ts = _FRESH_LATEST_TS
    total = flat_n + trend_n
    closes = [flat_close] * flat_n + [
        flat_close + (trend_end_close - flat_close) * (i / max(trend_n - 1, 1))
        for i in range(trend_n)
    ]
    bars = []
    for idx, c in enumerate(closes):
        ts = latest_ts - timedelta(minutes=(total - 1 - idx) * interval_minutes)
        bars.append(_bar(ts, c))
    return bars


_PASSING_LIQUIDITY_SIDECAR: dict[str, float] = {
    "latest_close": 190.0,
    "avg_daily_volume_shares": 50_000_000.0,
    "avg_daily_dollar_volume": 9_500_000_000.0,
    "relative_volume": 1.5,
    "spread_estimate_bps": 8.0,
    "slippage_estimate_bps": 6.0,
    "round_trip_cost_bps": 14.0,
}


def _make_ranked_watchlist(
    *,
    symbol: str = _SYMBOL,
    strategy_id: str = _STRATEGY_ID,
    regime_label: str = _REGIME,
    approved_for_live: bool = False,
) -> dict[str, Any]:
    """A ranked/selector-shaped watchlist-v1 candidate, prior to any promotion."""
    return {
        "schema_version": "watchlist-v1",
        "generated_at_utc": _NOW.isoformat(),
        "trade_date": _TRADE_DATE,
        "mode": "paper",
        "approved_for_autonomous_paper": False,
        "approved_for_live": approved_for_live,
        "max_symbols_to_trade": 1,
        "max_concurrent_positions": 1,
        "symbols": [symbol],
        "strategy_assignments": {symbol: strategy_id},
        "paper_qty_limits": {symbol: 1},
        "notional_limits": {symbol: 1000.0},
        "ranked_candidates": [
            {
                "rank": 1,
                "symbol": symbol,
                "strategy_id": strategy_id,
                "regime_label": regime_label,
                "regime_score": 0.82,
                "net_expectancy_after_cost_bps": 7.0,
            }
        ],
        "selection_reason": "paper_readiness_runner_test_fixture",
    }


def _raw_strategy_fit_metrics(
    *,
    symbol: str = _SYMBOL,
    strategy_id: str = _STRATEGY_ID,
    regime_label: str = _REGIME,
    **overrides: Any,
) -> dict[str, Any]:
    """Pre-gate strategy-fit-v1 metrics that satisfy every BacktestGateConfig
    threshold with deterministic margin (mirrors the proven end-to-end chain
    fixture — see test_scanner_watchlist_promotion_end_to_end_artifact_chain)."""
    base: dict[str, Any] = {
        "schema_version": "strategy-fit-v1",
        "generated_at_utc": _NOW.isoformat(),
        "symbol": symbol,
        "strategy_id": strategy_id,
        "regime_label": regime_label,
        "status": "complete",
        "bars_used": 300,
        "trades": 50,
        "win_rate": 0.60,
        "profit_factor": 1.5,
        "expectancy_bps": 10.0,
        "avg_trade_bps": 8.0,
        "max_drawdown_bps": 150.0,
        "sharpe": 1.2,
        "sortino": 1.5,
        "net_expectancy_after_cost_bps": 7.0,
        "sample_quality": 0.85,
        "parameter_stability_score": 0.80,
        "largest_trade_profit_fraction": 0.15,
        "validation_profit_factor": 1.2,
        "validation_trades": 15,
        "passed_min_bars": False,
        "passed_min_trades": False,
        "passed_max_drawdown": False,
        "passed_profit_factor": False,
        "passed_expectancy": False,
        "passed_cost_adjusted_edge": False,
        "passed_out_of_sample_check": False,
        "recommended_for_paper": False,
        "recommended_for_live": False,
        "failure_reasons": [],
        "notes": "deterministic fixture metrics for paper-readiness-runner proof",
    }
    base.update(overrides)
    return base


def _make_passing_strategy_fit(**overrides: Any) -> dict[str, Any]:
    """Run raw metrics through the real backtest_gates evaluator so that
    `recommended_for_paper` is derived honestly, not hand-set."""
    raw = _raw_strategy_fit_metrics(**overrides)
    return apply_backtest_gates(raw, BacktestGateConfig())


def _write_strategy_fit_artifact(strategy_fit_dir: Path, artifact: dict[str, Any]) -> Path:
    name = f"strategy_fit_{artifact['symbol']}_fixture.json"
    path = strategy_fit_dir / name
    _write_json(path, artifact)
    return path


def _build_operational_fixture(
    tmp_root: Path,
    *,
    symbol: str = _SYMBOL,
    write_bars: bool = True,
    bars: Optional[list[dict[str, Any]]] = None,
    write_liquidity: bool = True,
    write_strategy_fit: bool = True,
    fit_overrides: Optional[dict[str, Any]] = None,
    watchlist_overrides: Optional[dict[str, Any]] = None,
    **config_overrides: Any,
) -> PaperReadinessConfig:
    """Lay out a full local fixture tree (watchlist/bars/liquidity/strategy-fit)
    under `tmp_root` and return a `PaperReadinessConfig` pointing at it.

    Every toggle defaults ON and `operator_review_approved=True`, so the chain
    can run end to end unmodified; callers override individual knobs/files to
    deterministically exercise specific toggle/fail-closed/safety paths.
    """
    watchlist = _make_ranked_watchlist(symbol=symbol)
    if watchlist_overrides:
        watchlist.update(watchlist_overrides)
    watchlist_path = tmp_root / "watchlist.json"
    _write_json(watchlist_path, watchlist)

    bars_root = tmp_root / "bars"
    bars_root.mkdir(parents=True, exist_ok=True)
    if write_bars:
        _write_json(bars_root / f"{symbol}_5m.json", bars if bars is not None else _trending_up_bars())

    liquidity_root: Optional[Path] = None
    if write_liquidity:
        liquidity_root = tmp_root / "liquidity"
        _write_json(liquidity_root / f"{symbol}.json", _PASSING_LIQUIDITY_SIDECAR)

    strategy_fit_dir = tmp_root / "strategy_fit"
    strategy_fit_dir.mkdir(parents=True, exist_ok=True)
    if write_strategy_fit:
        fit = _make_passing_strategy_fit(symbol=symbol, **(fit_overrides or {}))
        _write_strategy_fit_artifact(strategy_fit_dir, fit)

    out_root = tmp_root / "out"
    base_kwargs: dict[str, Any] = dict(
        trade_date=_TRADE_DATE,
        scanner_pipeline_enabled=True,
        backtest_pipeline_enabled=True,
        symbol_inputs_enabled=True,
        watchlist_promotion_enabled=True,
        paper_readiness_enabled=True,
        paper_handoff_enabled=False,
        live_handoff_enabled=False,
        operator_review_approved=True,
        mode="paper",
        bars_root=str(bars_root),
        watchlist_path=str(watchlist_path),
        strategy_fit_dir=str(strategy_fit_dir),
        liquidity_root=str(liquidity_root) if liquidity_root else None,
        timeframe="5m",
        symbol_inputs_output_path=str(out_root / "symbol_inputs.json"),
        premarket_output_path=str(out_root / "premarket_revalidation.json"),
        promoted_watchlist_output_path=str(out_root / "promoted_watchlist.json"),
        readiness_report_output_path=str(out_root / "readiness_report.json"),
        now_utc=_NOW,
        generated_at_utc=_NOW.isoformat(),
    )
    base_kwargs.update(config_overrides)
    return PaperReadinessConfig(**base_kwargs)


# ===========================================================================
# 1-5: Toggle behavior
# ===========================================================================

class TestToggleLiveHandoffForbidden(unittest.TestCase):
    """1: live_handoff_enabled=True is an unconditional hard block — checked
    before mode/watchlist/anything else; the chain never runs."""

    def test_live_handoff_enabled_blocks_unconditionally(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), live_handoff_enabled=True)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_LIVE_HANDOFF_FORBIDDEN])
            self.assertFalse(result.approved_for_autonomous_paper)
            self.assertFalse(result.approved_for_live)
            self.assertIsNone(result.artifacts_written["symbol_inputs_output_path"])
            self.assertIsNone(result.artifacts_written["promoted_watchlist_output_path"])
            self.assertIsNone(result.artifacts_written["readiness_report_output_path"])


class TestToggleModeNotPaper(unittest.TestCase):
    """2: mode != 'paper' blocks before any chain stage runs."""

    def test_non_paper_mode_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), mode="live")
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_MODE_NOT_PAPER])
            self.assertFalse(result.approved_for_autonomous_paper)
            self.assertFalse(result.approved_for_live)


class TestTogglePaperReadinessDisabled(unittest.TestCase):
    """3: paper_readiness_enabled=False blocks before the watchlist is read —
    the master enable switch for the whole operational chain."""

    def test_paper_readiness_disabled_blocks_before_watchlist_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), paper_readiness_enabled=False)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_PAPER_READINESS_DISABLED])
            self.assertIsNone(result.top_symbol)


class TestToggleSymbolInputsDisabled(unittest.TestCase):
    """4: symbol_inputs_enabled=False blocks after the watchlist/bars/strategy
    fit resolve (top_symbol is known) but before the producer runs."""

    def test_symbol_inputs_disabled_blocks_before_producer_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), symbol_inputs_enabled=False)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_SYMBOL_INPUTS_DISABLED, result.reasons)
            self.assertEqual(result.top_symbol, _SYMBOL)
            self.assertIsNone(result.artifacts_written["symbol_inputs_output_path"])
            self.assertIsNone(result.artifacts_written["promoted_watchlist_output_path"])


class TestToggleWatchlistPromotionDisabled(unittest.TestCase):
    """5: watchlist_promotion_enabled=False blocks before the promotion
    evaluator runs (and before the symbol-inputs producer, since the disabled
    check is sequenced ahead of it)."""

    def test_watchlist_promotion_disabled_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), watchlist_promotion_enabled=False)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_WATCHLIST_PROMOTION_DISABLED, result.reasons)
            self.assertEqual(result.top_symbol, _SYMBOL)
            self.assertIsNone(result.artifacts_written["symbol_inputs_output_path"])
            self.assertIsNone(result.artifacts_written["promoted_watchlist_output_path"])


# ===========================================================================
# 6: Operational passing path
# ===========================================================================

class TestOperationalPassingPath(unittest.TestCase):
    """6: with every toggle enabled and operator review approved, the full
    chain runs end to end against real temp-dir fixtures and produces a
    paper-approved, live-locked result with every conventional artifact
    written to disk and reloadable."""

    def test_full_chain_reaches_ready_for_paper_handoff(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root)
            result = run_paper_readiness_pipeline(config)

            self.assertEqual(result.status, STATUS_READY_FOR_PAPER_HANDOFF, msg=result.notes)
            self.assertEqual(result.reasons, [])
            self.assertEqual(result.top_symbol, _SYMBOL)
            self.assertEqual(result.symbol_inputs_status, "complete")
            self.assertTrue(result.risk_simulation_passed)
            self.assertTrue(result.premarket_revalidation_passed)
            self.assertTrue(result.promotion_passed)
            self.assertTrue(result.approved_for_autonomous_paper)
            self.assertFalse(result.approved_for_live)
            self.assertTrue(result.live_locked)
            self.assertFalse(result.daemon_enforcement_executed)
            self.assertFalse(result.paper_handoff_requested)
            self.assertFalse(result.paper_handoff_executed)

            for key in (
                "symbol_inputs_output_path",
                "premarket_output_path",
                "promoted_watchlist_output_path",
                "readiness_report_output_path",
            ):
                written_path = result.artifacts_written[key]
                self.assertIsNotNone(written_path, msg=key)
                self.assertTrue(Path(written_path).is_file(), msg=key)

            promoted = json.loads(
                Path(result.artifacts_written["promoted_watchlist_output_path"]).read_text(encoding="utf-8")
            )
            self.assertTrue(promoted["approved_for_autonomous_paper"])
            self.assertFalse(promoted["approved_for_live"])
            self.assertEqual(promoted["mode"], "paper")
            self.assertEqual(promoted["max_symbols_to_trade"], 1)
            self.assertEqual(promoted["max_concurrent_positions"], 1)
            self.assertEqual(promoted["symbols"], [_SYMBOL])
            self.assertEqual(promoted["strategy_assignments"], {_SYMBOL: _STRATEGY_ID})

            report = json.loads(
                Path(result.artifacts_written["readiness_report_output_path"]).read_text(encoding="utf-8")
            )
            self.assertEqual(report["schema_version"], SCHEMA_VERSION)
            self.assertEqual(report["status"], STATUS_READY_FOR_PAPER_HANDOFF)
            self.assertTrue(report["approved_for_autonomous_paper"])
            self.assertFalse(report["approved_for_live"])
            self.assertTrue(report["live_locked"])
            self.assertFalse(report["daemon_enforcement_executed"])
            self.assertFalse(report["paper_handoff_executed"])


# ===========================================================================
# 7-15: Fail-closed paths
# ===========================================================================

class TestFailClosedWatchlistPathMissing(unittest.TestCase):
    """7: no watchlist_path configured blocks before any file IO."""

    def test_missing_watchlist_path_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = _build_operational_fixture(Path(tmp), watchlist_path=None)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_WATCHLIST_PATH_MISSING])
            self.assertIsNone(result.top_symbol)


class TestFailClosedWatchlistLoadFailed(unittest.TestCase):
    """8: a malformed or absent watchlist file fails closed with an explicit
    load-failure reason — never a crash, never a silently-empty watchlist."""

    def test_malformed_watchlist_json_blocks_with_load_failed(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root)
            Path(config.watchlist_path).write_text("{not valid json", encoding="utf-8")
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_WATCHLIST_LOAD_FAILED])

    def test_missing_watchlist_file_blocks_with_load_failed(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(
                tmp_root, watchlist_path=str(tmp_root / "does_not_exist.json")
            )
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.reasons, [REASON_WATCHLIST_LOAD_FAILED])


class TestFailClosedNoRankedCandidates(unittest.TestCase):
    """9: an empty symbols list in the watchlist blocks — never silently picks
    a default or fabricated candidate."""

    def test_empty_symbols_list_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root, watchlist_overrides={"symbols": []})
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_NO_RANKED_CANDIDATES, result.reasons)
            self.assertIsNone(result.top_symbol)


class TestFailClosedBarsRootMissing(unittest.TestCase):
    """10: a bars_root that does not exist (or is not a directory) blocks
    before the symbol-inputs producer is ever invoked."""

    def test_missing_bars_root_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root, bars_root=str(tmp_root / "no_such_bars_dir"))
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_BARS_ROOT_MISSING, result.reasons)
            self.assertEqual(result.top_symbol, _SYMBOL)
            self.assertIsNone(result.artifacts_written["symbol_inputs_output_path"])


class TestFailClosedStrategyFitDirMissing(unittest.TestCase):
    """11: a strategy_fit_dir that does not exist blocks before per-symbol
    strategy-fit presence is even evaluated."""

    def test_missing_strategy_fit_dir_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(
                tmp_root, strategy_fit_dir=str(tmp_root / "no_such_fit_dir")
            )
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_STRATEGY_FIT_DIR_MISSING, result.reasons)
            self.assertEqual(result.top_symbol, _SYMBOL)


class TestFailClosedStrategyFitMissingForTopSymbol(unittest.TestCase):
    """12: the strategy_fit_dir exists but contains no strategy-fit-v1
    artifact for the top-ranked symbol — fails closed rather than silently
    promoting on an absent fit record."""

    def test_no_strategy_fit_artifact_for_top_symbol_blocks(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root, write_strategy_fit=False)
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_STRATEGY_FIT_MISSING, result.reasons)
            self.assertEqual(result.top_symbol, _SYMBOL)


class TestFailClosedSymbolInputsRunnerBlocked(unittest.TestCase):
    """13: when the symbol-inputs producer itself cannot write its artifact
    (output path collides with an existing non-directory file), the chain
    blocks and surfaces both the wrapper reason and the upstream status."""

    def test_symbol_inputs_output_write_failure_blocks_chain(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            blocker_file = tmp_root / "symbol_inputs_blocker"
            blocker_file.write_text("not a directory", encoding="utf-8")
            config = _build_operational_fixture(
                tmp_root,
                symbol_inputs_output_path=str(blocker_file / "symbol_inputs.json"),
            )
            result = run_paper_readiness_pipeline(config)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_SYMBOL_INPUTS_BLOCKED, result.reasons)
            self.assertEqual(result.symbol_inputs_status, "blocked")
            self.assertIsNone(result.artifacts_written["symbol_inputs_output_path"])
            self.assertIsNone(result.artifacts_written["promoted_watchlist_output_path"])
            self.assertFalse(result.approved_for_autonomous_paper)


class TestFailClosedSymbolInputsPartialContinuesChain(unittest.TestCase):
    """14: when the symbol-inputs producer reports 'partial' (missing bars for
    a requested symbol), the chain continues and aggregates honestly — the
    final status reflects the partial upstream state rather than hiding it,
    and approval still requires every gate to genuinely pass."""

    def test_missing_bars_for_symbol_yields_partial_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root, write_bars=False)
            result = run_paper_readiness_pipeline(config)

            self.assertEqual(result.symbol_inputs_status, "partial")
            self.assertIn(REASON_SYMBOL_INPUTS_PARTIAL, result.reasons)
            self.assertEqual(result.status, STATUS_PARTIAL, msg=result.notes)
            # The stage still produced an artifact (fail-closed no-bars record)
            # — the partial path is never silently dropped.
            symbol_inputs_path = result.artifacts_written["symbol_inputs_output_path"]
            self.assertIsNotNone(symbol_inputs_path)
            self.assertTrue(Path(symbol_inputs_path).is_file())
            self.assertFalse(result.approved_for_autonomous_paper)
            self.assertFalse(result.approved_for_live)


class TestFailClosedOperatorReviewMissingYieldsReadyForReview(unittest.TestCase):
    """15: when every other gate genuinely passes and only operator sign-off
    is missing, the runner reports 'ready_for_operator_review' — distinct from
    a generic block — while keeping approved_for_autonomous_paper False until
    the operator explicitly approves."""

    def test_operator_review_missing_yields_ready_for_operator_review(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(tmp_root, operator_review_approved=False)
            result = run_paper_readiness_pipeline(config)

            self.assertEqual(result.status, STATUS_READY_FOR_OPERATOR_REVIEW, msg=result.notes)
            self.assertEqual(result.reasons, [REASON_OPERATOR_REVIEW_REQUIRED])
            self.assertFalse(result.promotion_passed)
            self.assertFalse(result.approved_for_autonomous_paper)
            self.assertFalse(result.approved_for_live)
            self.assertIsNotNone(result.artifacts_written["promoted_watchlist_output_path"])


# ===========================================================================
# 16-21: Safety invariants
# ===========================================================================

class TestSafetyConfigForcesApprovedForLiveFalse(unittest.TestCase):
    """16: PaperReadinessConfig.approved_for_live is forced False via
    __post_init__ regardless of what a caller constructs it with — matching
    the established hard-invariant pattern across every scanner config."""

    def test_config_forces_approved_for_live_false(self):
        config = PaperReadinessConfig(trade_date=_TRADE_DATE, approved_for_live=True)
        self.assertFalse(config.approved_for_live)


class TestSafetyResultForcesApprovedForLiveFalse(unittest.TestCase):
    """17: PaperReadinessResult.approved_for_live is forced False via
    __post_init__ — and stays False through to_dict() — regardless of caller
    input."""

    def test_result_forces_approved_for_live_false(self):
        result = PaperReadinessResult(approved_for_live=True)
        self.assertFalse(result.approved_for_live)
        self.assertFalse(result.to_dict()["approved_for_live"])


class TestSafetyForgedLiveApprovalNeverPropagates(unittest.TestCase):
    """18: a watchlist artifact forged with approved_for_live=True is reported
    via paper_readiness_forged_live_approval_forbidden — and the real
    promotion gate independently fails closed on it too — so neither the
    in-memory result nor the written promoted artifact ever shows
    approved_for_live=True or an approved symbol."""

    def test_forged_live_approval_is_reported_and_never_propagates(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config = _build_operational_fixture(
                tmp_root, watchlist_overrides={"approved_for_live": True},
            )
            result = run_paper_readiness_pipeline(config)

            self.assertIn(REASON_FORGED_LIVE_APPROVAL_FORBIDDEN, result.reasons)
            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertFalse(result.approved_for_live)
            self.assertFalse(result.approved_for_autonomous_paper)

            promoted_path = result.artifacts_written["promoted_watchlist_output_path"]
            self.assertIsNotNone(promoted_path)
            promoted = json.loads(Path(promoted_path).read_text(encoding="utf-8"))
            self.assertFalse(promoted["approved_for_live"])
            self.assertFalse(promoted["approved_for_autonomous_paper"])
            self.assertEqual(promoted["symbols"], [])


class TestSafetyResultLockAndExecutionFlagsAlwaysForced(unittest.TestCase):
    """19: live_locked / daemon_enforcement_executed / paper_handoff_executed
    are hard invariants on PaperReadinessResult — never overrideable, and
    propagate unchanged through to_dict()."""

    def test_lock_and_execution_flags_are_forced(self):
        result = PaperReadinessResult(
            live_locked=False,
            daemon_enforcement_executed=True,
            paper_handoff_executed=True,
        )
        self.assertTrue(result.live_locked)
        self.assertFalse(result.daemon_enforcement_executed)
        self.assertFalse(result.paper_handoff_executed)

        as_dict = result.to_dict()
        self.assertTrue(as_dict["live_locked"])
        self.assertFalse(as_dict["daemon_enforcement_executed"])
        self.assertFalse(as_dict["paper_handoff_executed"])


class TestSafetyNoForbiddenImportsOrReferences(unittest.TestCase):
    """20: the runner module is a pure local-artifact composition layer — no
    broker/OMS/daemon/runtime/network/DB/subprocess references, and no
    assignment anywhere sets approved_for_live to True."""

    @classmethod
    def setUpClass(cls):
        cls.source = Path(prr_module.__file__).read_text(encoding="utf-8")

    def test_no_network_imports(self):
        for forbidden in ("import requests", "import urllib", "import http.client", "import aiohttp", "import httpx"):
            self.assertNotIn(forbidden, self.source, msg=forbidden)

    def test_no_db_imports(self):
        for forbidden in ("import psycopg", "import sqlalchemy", "import asyncpg", "from mqk_db", "import mqk_db"):
            self.assertNotIn(forbidden, self.source, msg=forbidden)

    def test_no_daemon_runtime_or_broker_references(self):
        for forbidden in (
            "from mqk_daemon", "import mqk_daemon",
            "from mqk_runtime", "import mqk_runtime",
            "from mqk_execution", "import mqk_execution",
            "BrokerGateway", "broker_adapter", "submit_order", "place_order",
            "/v2/orders", "oms_outbox", "oms_inbox", "strategy/signal",
        ):
            self.assertNotIn(forbidden, self.source, msg=forbidden)

    def test_no_subprocess(self):
        self.assertNotIn("import subprocess", self.source)
        self.assertNotIn("os.system", self.source)

    def test_no_approved_for_live_true_assignment(self):
        self.assertNotIn('approved_for_live"] = True', self.source)
        self.assertNotIn("approved_for_live: bool = True", self.source)
        self.assertNotIn("approved_for_live = True", self.source)


class TestSafetyConfigLoaderCannotBeLiveSideChannel(unittest.TestCase):
    """21: load_config_from_json only honors known declared fields, requires a
    non-empty trade_date, ignores runtime-only fields (now_utc etc.), and
    forces approved_for_live=False even when the serialized JSON tries to set
    it True — config loading cannot be used to enable live trading."""

    def test_loader_forces_invariants_and_ignores_runtime_only_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config_path = tmp_root / "config.json"
            _write_json(config_path, {
                "trade_date": _TRADE_DATE,
                "paper_readiness_enabled": True,
                "symbol_inputs_enabled": True,
                "watchlist_promotion_enabled": True,
                "operator_review_approved": True,
                "mode": "paper",
                "watchlist_path": str(tmp_root / "watchlist.json"),
                "bars_root": str(tmp_root / "bars"),
                "strategy_fit_dir": str(tmp_root / "strategy_fit"),
                "approved_for_live": True,
                "now_utc": "2026-06-08T14:00:00+00:00",
                "unknown_future_field": "ignored",
            })
            config = load_config_from_json(str(config_path))
            self.assertFalse(config.approved_for_live)
            self.assertIsNone(config.now_utc)
            self.assertTrue(config.paper_readiness_enabled)
            self.assertTrue(config.operator_review_approved)
            self.assertEqual(config.trade_date, _TRADE_DATE)

    def test_loader_raises_on_missing_trade_date(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config_path = tmp_root / "config.json"
            _write_json(config_path, {"paper_readiness_enabled": True})
            with self.assertRaises(ValueError) as ctx:
                load_config_from_json(str(config_path))
            self.assertIn(REASON_CONFIG_LOAD_FAILED, str(ctx.exception))

    def test_loader_raises_on_malformed_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            config_path = tmp_root / "config.json"
            config_path.write_text("{not valid json", encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                load_config_from_json(str(config_path))
            self.assertIn(REASON_CONFIG_LOAD_FAILED, str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
