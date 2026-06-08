"""
WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01 — full research-layer artifact
chain proof.

Proves the complete artifact chain consumes backtest outputs correctly to
produce a paper-approved watchlist artifact:

    ranked/scanner candidate (watchlist-v1)
      -> backtest queue (regime/strategy compatibility)
      -> strategy-fit-v1 (evaluated through the real backtest_gates evaluator)
      -> backtest gates (recommended_for_paper derived honestly)
      -> risk simulation (evaluate_watchlist_risk)
      -> symbol-inputs runner (run_symbol_inputs_producer, real temp-dir bars)
      -> premarket revalidation (evaluate_premarket_watchlist)
      -> watchlist promotion (evaluate_watchlist_promotion / apply)
      -> promoted paper watchlist artifact (written to a temp dir)

Artifact-only proof. Deterministic in-memory + temp-dir fixtures only. No
daemon/broker/DB/network/subprocess imports; no order submission; no strategy
signal injection; no live routing. `approved_for_live` is always False —
`approved_for_autonomous_paper` is the only promotable surface in v1.

This test does NOT prove market/live/paper truth — it proves that the
research-layer modules wire together honestly and fail closed when any link
in the chain is missing, mismatched, forged, or stale.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.backtest_queue import strategy_ids_for_regime
from mqk_research.scanner.backtest_gates import BacktestGateConfig, apply_backtest_gates
from mqk_research.scanner.risk_simulation import (
    SCHEMA_VERSION as RISK_SIMULATION_SCHEMA_VERSION,
    RiskSimulationConfig,
    evaluate_watchlist_risk,
)
from mqk_research.scanner.watchlist_promotion import (
    SCHEMA_VERSION_WATCHLIST,
    REASON_STRATEGY_FIT_MISSING,
    REASON_STRATEGY_FIT_SYMBOL_MISMATCH,
    REASON_STRATEGY_FIT_STRATEGY_MISMATCH,
    REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER,
    REASON_RISK_SIMULATION_REQUIRED,
    REASON_OPERATOR_REVIEW_REQUIRED,
    REASON_PREMARKET_REVALIDATION_REQUIRED,
    REASON_LIVE_APPROVAL_FORBIDDEN,
    WatchlistPromotionConfig,
    evaluate_watchlist_promotion,
    apply_watchlist_promotion,
    write_promoted_watchlist,
)
from mqk_research.scanner.symbol_inputs_runner import (
    SymbolInputsRunnerConfig,
    run_symbol_inputs_producer,
)
from mqk_research.scanner.symbol_inputs import (
    SCHEMA_VERSION as SYMBOL_INPUTS_SCHEMA_VERSION,
    load_symbol_inputs_artifact,
    extract_symbol_inputs_map,
)
from mqk_research.scanner.premarket_revalidation import (
    SCHEMA_VERSION as PREMARKET_SCHEMA_VERSION,
    REASON_PREMARKET_SYMBOL_INPUT_MISSING,
    REASON_PREMARKET_BAR_STALE,
    PremarketRevalidationConfig,
    evaluate_premarket_watchlist,
    apply_premarket_revalidation_to_watchlist,
)


# ---------------------------------------------------------------------------
# Deterministic fixtures — symbols, time, bars, liquidity
# ---------------------------------------------------------------------------

_NOW = datetime(2026, 6, 8, 14, 0, 0, tzinfo=timezone.utc)
_TRADE_DATE = "2026-06-08"
_SYMBOL = "AAPL"
_STRATEGY_ID = "intraday_scalper"
_REGIME = "trending_up"

_FRESH_LATEST_TS = _NOW - timedelta(minutes=5)     # within 20-min freshness window
_STALE_LATEST_TS = _NOW - timedelta(minutes=90)    # well beyond 20-min freshness window


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

    Mirrors the proven fixture pattern from test_scanner_symbol_inputs(_runner).py
    that classifies as trending_up and passes data-quality + regime gates.
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


# ---------------------------------------------------------------------------
# Stage 1: ranked/scanner candidate (watchlist-v1, pre-promotion)
# ---------------------------------------------------------------------------

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
        "selection_reason": "ranked_candidate_artifact_chain_proof",
    }


# ---------------------------------------------------------------------------
# Stage 2-3: strategy-fit-v1, evaluated honestly through the real backtest_gates
# evaluator (recommended_for_paper is *derived*, not hand-set).
# ---------------------------------------------------------------------------

def _raw_strategy_fit_metrics(
    *,
    symbol: str = _SYMBOL,
    strategy_id: str = _STRATEGY_ID,
    regime_label: str = _REGIME,
    **overrides: Any,
) -> dict[str, Any]:
    """Pre-gate strategy-fit-v1 metrics shaped like real backtest_runner output,
    satisfying every BacktestGateConfig threshold with deterministic margin:
      bars_used=300>=200, trades=50>=30, profit_factor=1.5>=1.3,
      expectancy_bps=10.0>0, net_expectancy_after_cost_bps=7.0>=5,
      max_drawdown_bps=150<=800 (and small enough to also pass risk-sim
      max_daily_loss_bps=200 and stressed-drawdown checks downstream),
      validation_profit_factor=1.2>=1.1, validation_trades=15>=10,
      sample_quality=0.85>=0.5, parameter_stability_score=0.80>=0.7,
      largest_trade_profit_fraction=0.15<=0.30.
    """
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
        "notes": "deterministic fixture metrics for end-to-end chain proof",
    }
    base.update(overrides)
    return base


def _make_passing_strategy_fit(**overrides: Any) -> dict[str, Any]:
    """Run raw metrics through the real backtest_gates evaluator so that
    `recommended_for_paper` is derived honestly — proving promotion consumes a
    genuinely gate-evaluated artifact, not a hand-forged boolean."""
    raw = _raw_strategy_fit_metrics(**overrides)
    return apply_backtest_gates(raw, BacktestGateConfig())


def _make_not_recommended_strategy_fit(**overrides: Any) -> dict[str, Any]:
    """A strategy-fit artifact that genuinely fails backtest gates
    (trades=5 < min_trades=30) -> recommended_for_paper=False, derived honestly."""
    raw = _raw_strategy_fit_metrics(trades=5, **overrides)
    return apply_backtest_gates(raw, BacktestGateConfig())


# ---------------------------------------------------------------------------
# Stage 4: risk simulation
# ---------------------------------------------------------------------------

def _passing_risk_config() -> RiskSimulationConfig:
    return RiskSimulationConfig()


# ---------------------------------------------------------------------------
# Stage 5: symbol-inputs runner — real producer run against temp-dir bars
# ---------------------------------------------------------------------------

def _run_symbol_inputs_chain(
    tmp_root: Path,
    *,
    symbol: str = _SYMBOL,
    bars: Optional[list[dict[str, Any]]] = None,
    write_bars: bool = True,
    with_liquidity: bool = True,
    now_utc: Optional[datetime] = None,
):
    """Run the real `run_symbol_inputs_producer` against a temp bars/liquidity
    root and return the structured result. `bars=None` + `write_bars=True`
    writes the proven trending_up fixture; `write_bars=False` leaves the bars
    root empty (symbol input becomes a fail-closed no-bars record)."""
    bars_root = tmp_root / "bars"
    bars_root.mkdir(parents=True, exist_ok=True)
    if write_bars:
        _write_json(bars_root / f"{symbol}_5m.json", bars if bars is not None else _trending_up_bars())

    liquidity_root: Optional[Path] = None
    if with_liquidity:
        liquidity_root = tmp_root / "liquidity"
        _write_json(liquidity_root / f"{symbol}.json", _PASSING_LIQUIDITY_SIDECAR)

    output_path = tmp_root / "out" / "symbol_inputs.json"
    config = SymbolInputsRunnerConfig(
        trade_date=_TRADE_DATE,
        output_path=str(output_path),
        bars_root=str(bars_root),
        timeframe="5m",
        symbols=[symbol],
        liquidity_root=str(liquidity_root) if liquidity_root else None,
        source="e2e_chain_proof",
        now_utc=now_utc if now_utc is not None else _NOW,
    )
    result = run_symbol_inputs_producer(config)
    return result, output_path


# ---------------------------------------------------------------------------
# Stage 6: premarket revalidation
# ---------------------------------------------------------------------------

def _passing_premarket_config() -> PremarketRevalidationConfig:
    return PremarketRevalidationConfig()


# ---------------------------------------------------------------------------
# Stage 7: watchlist promotion
# ---------------------------------------------------------------------------

def _operator_approved_promotion_config(*, premarket_required: bool = False) -> WatchlistPromotionConfig:
    return WatchlistPromotionConfig(
        operator_review_approved=True,
        risk_simulation_passed=True,
        premarket_revalidation_required=premarket_required,
    )


# ===========================================================================
# A. Passing artifact-chain path
# ===========================================================================

class TestPassingArtifactChain(unittest.TestCase):
    """Walks the full chain end to end and asserts each stage consumes the
    previous stage's real output honestly, ending in a promoted paper
    watchlist with approved_for_autonomous_paper=True and approved_for_live=False."""

    def test_full_chain_produces_approved_paper_watchlist(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)

            # --- Stage 1: ranked candidate -------------------------------------
            watchlist = _make_ranked_watchlist()
            self.assertEqual(watchlist["schema_version"], SCHEMA_VERSION_WATCHLIST)
            self.assertEqual(watchlist["mode"], "paper")
            self.assertFalse(watchlist["approved_for_live"])
            self.assertFalse(watchlist["approved_for_autonomous_paper"])
            top_symbol = watchlist["symbols"][0]
            self.assertEqual(top_symbol, _SYMBOL)

            # --- Stage 2: backtest queue — regime/strategy compatibility -------
            # Logically links the ranked candidate's regime+assignment to the
            # same compatibility matrix the real backtest queue uses to decide
            # which strategies get queued for this regime.
            candidate = watchlist["ranked_candidates"][0]
            compatible_strategies = strategy_ids_for_regime(candidate["regime_label"])
            self.assertIn(_STRATEGY_ID, compatible_strategies)
            self.assertEqual(watchlist["strategy_assignments"][top_symbol], _STRATEGY_ID)

            # --- Stage 3: strategy-fit, evaluated through real backtest_gates ---
            fit_artifact = _make_passing_strategy_fit()
            self.assertEqual(fit_artifact["schema_version"], "strategy-fit-v1")
            self.assertEqual(fit_artifact["symbol"], _SYMBOL)
            self.assertEqual(fit_artifact["strategy_id"], _STRATEGY_ID)
            self.assertEqual(fit_artifact["regime_label"], _REGIME)
            self.assertGreaterEqual(fit_artifact["bars_used"], 200)
            self.assertGreaterEqual(fit_artifact["trades"], 30)
            self.assertGreaterEqual(fit_artifact["profit_factor"], 1.3)
            self.assertGreater(fit_artifact["expectancy_bps"], 0)
            self.assertGreaterEqual(fit_artifact["net_expectancy_after_cost_bps"], 5.0)
            self.assertLessEqual(fit_artifact["max_drawdown_bps"], 800.0)
            self.assertGreaterEqual(fit_artifact["validation_profit_factor"], 1.1)
            self.assertGreaterEqual(fit_artifact["validation_trades"], 10)
            self.assertGreaterEqual(fit_artifact["sample_quality"], 0.5)
            self.assertGreaterEqual(fit_artifact["parameter_stability_score"], 0.7)
            self.assertLessEqual(fit_artifact["largest_trade_profit_fraction"], 0.30)
            self.assertEqual(fit_artifact["failure_reasons"], [])
            # Derived (not hand-set) — proves the gate evaluator was actually run.
            self.assertTrue(fit_artifact["recommended_for_paper"])
            self.assertFalse(fit_artifact["recommended_for_live"])
            fit_artifacts = {top_symbol: fit_artifact}

            # --- Stage 4: risk simulation ---------------------------------------
            risk_result = evaluate_watchlist_risk(watchlist, fit_artifacts, _passing_risk_config())
            self.assertTrue(risk_result.passed, msg=risk_result.notes)
            self.assertEqual(risk_result.failure_reasons, [])
            self.assertTrue(risk_result.check_live_lock)
            self.assertTrue(risk_result.check_has_candidates)
            self.assertTrue(risk_result.check_strategy_fit_present)
            self.assertTrue(risk_result.check_concentration)
            self.assertTrue(risk_result.check_max_position_notional)
            self.assertTrue(risk_result.check_max_daily_loss)
            self.assertTrue(risk_result.check_drawdown_stress)
            self.assertTrue(risk_result.check_regime_consistency)
            self.assertTrue(risk_result.check_cost_adjusted_edge)
            self.assertEqual(risk_result.top_symbol, top_symbol)
            risk_dict = risk_result.to_dict()
            self.assertEqual(risk_dict["schema_version"], RISK_SIMULATION_SCHEMA_VERSION)
            self.assertEqual(risk_dict["schema_version"], "risk-simulation-v1")

            # --- Stage 5: symbol-inputs runner (real producer, temp-dir bars) ---
            runner_result, symbol_inputs_path = _run_symbol_inputs_chain(tmp_root)
            self.assertEqual(runner_result.status, "complete", msg=runner_result.notes)
            self.assertEqual(runner_result.symbols_written, [_SYMBOL])
            self.assertEqual(runner_result.failure_reasons, [])
            self.assertFalse(runner_result.watchlist_live_approval_forbidden)
            self.assertFalse(runner_result.approved_for_live)
            self.assertTrue(symbol_inputs_path.is_file())

            symbol_inputs_artifact = load_symbol_inputs_artifact(str(symbol_inputs_path))
            self.assertEqual(symbol_inputs_artifact["schema_version"], SYMBOL_INPUTS_SCHEMA_VERSION)
            self.assertEqual(symbol_inputs_artifact["schema_version"], "symbol-inputs-v1")
            self.assertFalse(symbol_inputs_artifact["approved_for_live"])
            symbol_inputs_map = extract_symbol_inputs_map(symbol_inputs_artifact)
            self.assertIn(_SYMBOL, symbol_inputs_map)
            sym_record = symbol_inputs_map[_SYMBOL]
            self.assertTrue(sym_record["data_quality_passed"], msg=sym_record.get("rejection_reason"))
            self.assertTrue(sym_record["liquidity_passed"], msg=sym_record.get("rejection_reason"))
            self.assertEqual(sym_record["regime_label"], _REGIME)
            self.assertIsNotNone(sym_record["latest_bar_ts"])
            self.assertIsNotNone(sym_record["latest_close"])

            # --- Stage 6: premarket revalidation ---------------------------------
            premarket_result = evaluate_premarket_watchlist(
                watchlist, symbol_inputs_map, _passing_premarket_config(), reference_utc=_NOW
            )
            self.assertTrue(premarket_result.passed, msg=premarket_result.notes)
            self.assertEqual(premarket_result.failure_reasons, [])
            self.assertEqual(premarket_result.top_symbol, top_symbol)
            premarket_dict = premarket_result.to_dict()
            self.assertEqual(premarket_dict["schema_version"], PREMARKET_SCHEMA_VERSION)
            self.assertEqual(premarket_dict["schema_version"], "premarket-revalidation-v1")
            premarket_applied = apply_premarket_revalidation_to_watchlist(watchlist, premarket_result)
            self.assertFalse(premarket_applied["approved_for_live"])

            # --- Stage 7: watchlist promotion ------------------------------------
            promotion_config = _operator_approved_promotion_config(premarket_required=False)
            decision = evaluate_watchlist_promotion(
                watchlist,
                fit_artifacts,
                promotion_config,
                risk_simulation_result=risk_result.to_dict(),
                premarket_revalidation_result=premarket_result.to_dict(),
            )
            self.assertTrue(decision.passed, msg=decision.notes)
            self.assertEqual(decision.failure_reasons, [])
            self.assertTrue(decision.approved_for_autonomous_paper)
            self.assertFalse(decision.approved_for_live)
            self.assertEqual(decision.approved_symbols, [_SYMBOL])
            self.assertEqual(decision.strategy_assignments, {_SYMBOL: _STRATEGY_ID})

            # --- Stage 8: final promoted paper watchlist artifact ----------------
            promoted = apply_watchlist_promotion(watchlist, decision, promotion_config)
            self.assertTrue(promoted["approved_for_autonomous_paper"])
            self.assertFalse(promoted["approved_for_live"])
            self.assertEqual(promoted["mode"], "paper")
            self.assertEqual(promoted["max_symbols_to_trade"], 1)
            self.assertEqual(promoted["max_concurrent_positions"], 1)
            self.assertEqual(promoted["symbols"], [_SYMBOL])
            self.assertEqual(promoted["strategy_assignments"], {_SYMBOL: _STRATEGY_ID})
            self.assertLessEqual(promoted["notional_limits"][_SYMBOL], watchlist["notional_limits"][_SYMBOL])
            self.assertIn("promotion_decision", promoted)
            self.assertTrue(promoted["promotion_decision"]["passed"])

            out_path = write_promoted_watchlist(promoted, str(tmp_root / "promoted" / "watchlist.json"))
            self.assertTrue(out_path.is_file())
            reloaded = json.loads(out_path.read_text(encoding="utf-8"))
            self.assertTrue(reloaded["approved_for_autonomous_paper"])
            self.assertFalse(reloaded["approved_for_live"])
            self.assertEqual(reloaded["mode"], "paper")
            self.assertEqual(reloaded["max_symbols_to_trade"], 1)
            self.assertEqual(reloaded["max_concurrent_positions"], 1)
            self.assertEqual(reloaded["strategy_assignments"], {_SYMBOL: _STRATEGY_ID})


# ===========================================================================
# B. Fail-closed paths
# ===========================================================================

class TestFailClosedMissingStrategyFit(unittest.TestCase):
    """B1: Missing strategy-fit artifact blocks promotion."""

    def test_missing_strategy_fit_blocks_promotion(self):
        watchlist = _make_ranked_watchlist()
        decision = evaluate_watchlist_promotion(
            watchlist,
            {},  # no strategy-fit for AAPL
            _operator_approved_promotion_config(),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)
        self.assertIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])


class TestFailClosedStrategyFitNotRecommended(unittest.TestCase):
    """B2: strategy-fit recommended_for_paper=False blocks promotion with the
    existing strategy-fit failure reason surfaced."""

    def test_not_recommended_for_paper_blocks_promotion(self):
        watchlist = _make_ranked_watchlist()
        fit = _make_not_recommended_strategy_fit()
        self.assertFalse(fit["recommended_for_paper"])
        self.assertIn("min_trades_failed", fit["failure_reasons"])

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: fit},
            _operator_approved_promotion_config(),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])


class TestFailClosedRiskSimulationFails(unittest.TestCase):
    """B3: risk simulation failure blocks promotion; final watchlist stays
    not approved."""

    def test_risk_simulation_failure_blocks_promotion(self):
        watchlist = _make_ranked_watchlist()
        # max_drawdown_bps=900 fails backtest gate max_drawdown (<=800), so the
        # fit is not recommended for paper either — but the relevant proof here
        # is that a *failing* risk_simulation_result independently blocks via
        # REASON_RISK_SIMULATION_REQUIRED. Use a fit that passes backtest gates
        # but trips the risk-sim daily-loss/drawdown-stress checks instead.
        fit = _make_passing_strategy_fit(max_drawdown_bps=300.0)
        self.assertTrue(fit["recommended_for_paper"])

        risk_result = evaluate_watchlist_risk(watchlist, {_SYMBOL: fit}, _passing_risk_config())
        self.assertFalse(risk_result.passed)
        self.assertIn("risk_daily_loss_failed", risk_result.failure_reasons)

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: fit},
            _operator_approved_promotion_config(),
            risk_simulation_result=risk_result.to_dict(),
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)

        promoted = apply_watchlist_promotion(watchlist, decision)
        self.assertFalse(promoted["approved_for_autonomous_paper"])
        self.assertFalse(promoted["approved_for_live"])
        self.assertEqual(promoted["symbols"], [])


class TestFailClosedOperatorReviewMissing(unittest.TestCase):
    """B4: missing operator review sign-off blocks promotion; final watchlist
    stays not approved."""

    def test_operator_review_missing_blocks_promotion(self):
        watchlist = _make_ranked_watchlist()
        fit = _make_passing_strategy_fit()
        config = WatchlistPromotionConfig(
            operator_review_approved=False,  # explicit — not signed off
            risk_simulation_passed=True,
            premarket_revalidation_required=False,
        )
        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: fit},
            config,
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_OPERATOR_REVIEW_REQUIRED, decision.failure_reasons)

        promoted = apply_watchlist_promotion(watchlist, decision, config)
        self.assertFalse(promoted["approved_for_autonomous_paper"])
        self.assertFalse(promoted["approved_for_live"])


class TestFailClosedPremarketMissingSymbolInput(unittest.TestCase):
    """B5: premarket fails closed when the symbol-inputs runner produces no
    bars for the symbol (missing symbol input record content), and that
    failure propagates to block promotion."""

    def test_missing_symbol_input_fails_premarket_and_promotion(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            watchlist = _make_ranked_watchlist()
            fit = _make_passing_strategy_fit()

            # Run the real runner with NO bars file written for AAPL — the
            # symbol still gets a fail-closed no-bars record (status=partial).
            runner_result, symbol_inputs_path = _run_symbol_inputs_chain(
                tmp_root, write_bars=False
            )
            self.assertEqual(runner_result.status, "partial")
            self.assertIn(_SYMBOL, runner_result.missing_bars)

            symbol_inputs_artifact = load_symbol_inputs_artifact(str(symbol_inputs_path))
            symbol_inputs_map = extract_symbol_inputs_map(symbol_inputs_artifact)
            self.assertIn(_SYMBOL, symbol_inputs_map)
            self.assertFalse(symbol_inputs_map[_SYMBOL]["data_quality_passed"])

            premarket_result = evaluate_premarket_watchlist(
                watchlist, symbol_inputs_map, _passing_premarket_config(), reference_utc=_NOW
            )
            self.assertFalse(premarket_result.passed)
            self.assertIn("premarket_data_quality_failed", premarket_result.failure_reasons)

            decision = evaluate_watchlist_promotion(
                watchlist,
                {_SYMBOL: fit},
                _operator_approved_promotion_config(premarket_required=True),
                risk_simulation_result={"passed": True},
                premarket_revalidation_result=premarket_result.to_dict(),
            )
            self.assertFalse(decision.passed)
            self.assertFalse(decision.approved_for_autonomous_paper)
            self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)

    def test_completely_absent_symbol_input_fails_premarket_and_promotion(self):
        """B5b: symbol absent from the symbol_inputs map entirely (e.g. runner
        was never pointed at this symbol) -> premarket_symbol_input_missing."""
        watchlist = _make_ranked_watchlist()
        fit = _make_passing_strategy_fit()

        premarket_result = evaluate_premarket_watchlist(
            watchlist, {}, _passing_premarket_config(), reference_utc=_NOW
        )
        self.assertFalse(premarket_result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, premarket_result.failure_reasons)

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: fit},
            _operator_approved_promotion_config(premarket_required=True),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result=premarket_result.to_dict(),
        )
        self.assertFalse(decision.passed)
        self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)


class TestFailClosedPremarketStaleBar(unittest.TestCase):
    """B6: premarket fails closed when time has moved on and the latest bar
    captured by the symbol-inputs producer is now stale relative to the
    premarket revalidation reference time — even though the producer itself
    saw the bar as fresh when it ran. This is the realistic "time has passed
    since the artifact was produced" revalidation scenario, distinct from B5
    (symbol input missing entirely). The stale-bar failure propagates to
    block promotion."""

    def test_stale_bar_fails_premarket_and_promotion(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            watchlist = _make_ranked_watchlist()
            fit = _make_passing_strategy_fit()

            # The bar was fresh when the symbol-inputs producer ran (5 minutes
            # after the bar closed) — data_quality_passed is honestly True.
            producer_now = _STALE_LATEST_TS + timedelta(minutes=5)
            bars = _trending_up_bars(latest_ts=_STALE_LATEST_TS)
            runner_result, symbol_inputs_path = _run_symbol_inputs_chain(
                tmp_root, bars=bars, now_utc=producer_now
            )
            self.assertEqual(runner_result.status, "complete")

            symbol_inputs_artifact = load_symbol_inputs_artifact(str(symbol_inputs_path))
            symbol_inputs_map = extract_symbol_inputs_map(symbol_inputs_artifact)
            self.assertTrue(symbol_inputs_map[_SYMBOL]["data_quality_passed"])
            self.assertEqual(symbol_inputs_map[_SYMBOL]["latest_bar_ts"], _STALE_LATEST_TS.isoformat())

            # By the time premarket revalidation runs (reference_utc=_NOW, 90
            # minutes after the bar), the same bar is now stale — revalidation
            # fails closed even though the original artifact looked fine.
            premarket_result = evaluate_premarket_watchlist(
                watchlist, symbol_inputs_map, _passing_premarket_config(), reference_utc=_NOW
            )
            self.assertFalse(premarket_result.passed)
            self.assertIn(REASON_PREMARKET_BAR_STALE, premarket_result.failure_reasons)

            decision = evaluate_watchlist_promotion(
                watchlist,
                {_SYMBOL: fit},
                _operator_approved_promotion_config(premarket_required=True),
                risk_simulation_result={"passed": True},
                premarket_revalidation_result=premarket_result.to_dict(),
            )
            self.assertFalse(decision.passed)
            self.assertFalse(decision.approved_for_autonomous_paper)
            self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)


class TestFailClosedForgedLiveFlag(unittest.TestCase):
    """B7: a forged approved_for_live=true on the input watchlist is forced
    false everywhere downstream, and the live-lock/forbidden reasons are
    recorded. No artifact in the chain may carry approved_for_live=true."""

    def test_forged_live_flag_forced_false_and_recorded(self):
        watchlist = _make_ranked_watchlist(approved_for_live=True)
        fit = _make_passing_strategy_fit()

        risk_result = evaluate_watchlist_risk(watchlist, {_SYMBOL: fit}, _passing_risk_config())
        self.assertFalse(risk_result.passed)
        self.assertIn("risk_live_approval_forbidden", risk_result.failure_reasons)
        self.assertFalse(risk_result.to_dict().get("approved_for_live", False))

        premarket_result = evaluate_premarket_watchlist(
            watchlist, {_SYMBOL: {"data_quality_passed": True, "liquidity_passed": True,
                                  "regime_label": _REGIME, "latest_bar_ts": _FRESH_LATEST_TS.isoformat()}},
            _passing_premarket_config(), reference_utc=_NOW,
        )
        self.assertFalse(premarket_result.passed)
        self.assertIn("premarket_live_approval_forbidden", premarket_result.failure_reasons)

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: fit},
            _operator_approved_promotion_config(),
            risk_simulation_result=risk_result.to_dict(),
            premarket_revalidation_result=premarket_result.to_dict(),
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)
        self.assertIn(REASON_LIVE_APPROVAL_FORBIDDEN, decision.failure_reasons)

        promoted = apply_watchlist_promotion(watchlist, decision)
        self.assertFalse(promoted["approved_for_live"])
        self.assertFalse(promoted["approved_for_autonomous_paper"])

        # Defense in depth: every artifact dict touched in this scenario is
        # asserted to never carry approved_for_live=True downstream.
        for artifact in (risk_result.to_dict(), premarket_result.to_dict(), promoted):
            self.assertIsNot(artifact.get("approved_for_live"), True)


class TestFailClosedMismatchedStrategy(unittest.TestCase):
    """B8: watchlist assigns AAPL to intraday_scalper, but the strategy-fit
    artifact for AAPL is for a different strategy (mean_reversion) ->
    promotion fails closed rather than silently adopting the wrong assignment."""

    def test_mismatched_strategy_blocks_promotion(self):
        watchlist = _make_ranked_watchlist(strategy_id=_STRATEGY_ID)
        self.assertEqual(watchlist["strategy_assignments"][_SYMBOL], _STRATEGY_ID)

        wrong_strategy_fit = _make_passing_strategy_fit(
            strategy_id="mean_reversion", regime_label="range_bound",
        )
        self.assertEqual(wrong_strategy_fit["symbol"], _SYMBOL)
        self.assertEqual(wrong_strategy_fit["strategy_id"], "mean_reversion")
        self.assertTrue(wrong_strategy_fit["recommended_for_paper"])

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: wrong_strategy_fit},
            _operator_approved_promotion_config(),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_STRATEGY_MISMATCH, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])
        self.assertEqual(decision.strategy_assignments, {})

        promoted = apply_watchlist_promotion(watchlist, decision)
        self.assertFalse(promoted["approved_for_autonomous_paper"])
        self.assertEqual(promoted["strategy_assignments"], {})


class TestFailClosedMismatchedSymbol(unittest.TestCase):
    """B9: the strategy-fit artifact registered for AAPL actually carries an
    internal `symbol` field for a different ticker (MSFT) — a forged/corrupted
    identity binding -> promotion fails closed rather than trusting the dict
    key alone."""

    def test_mismatched_symbol_identity_blocks_promotion(self):
        watchlist = _make_ranked_watchlist(symbol=_SYMBOL)
        self.assertEqual(watchlist["symbols"], [_SYMBOL])

        forged_identity_fit = _make_passing_strategy_fit(symbol="MSFT")
        self.assertEqual(forged_identity_fit["symbol"], "MSFT")
        self.assertTrue(forged_identity_fit["recommended_for_paper"])

        decision = evaluate_watchlist_promotion(
            watchlist,
            {_SYMBOL: forged_identity_fit},  # registered under AAPL's key
            _operator_approved_promotion_config(),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_SYMBOL_MISMATCH, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

        promoted = apply_watchlist_promotion(watchlist, decision)
        self.assertFalse(promoted["approved_for_autonomous_paper"])
        self.assertEqual(promoted["symbols"], [])

    def test_watchlist_keyed_to_other_symbol_entirely_fails_closed(self):
        """B9b: the chain hands the promotion gate a fit-artifacts map that
        only contains an entry for a different symbol than the watchlist's
        top candidate -> strategy_fit_missing for the actual top symbol."""
        watchlist = _make_ranked_watchlist(symbol=_SYMBOL)
        other_symbol_fit = _make_passing_strategy_fit(symbol="MSFT", strategy_id=_STRATEGY_ID)

        decision = evaluate_watchlist_promotion(
            watchlist,
            {"MSFT": other_symbol_fit},  # AAPL not present
            _operator_approved_promotion_config(),
            risk_simulation_result={"passed": True},
            premarket_revalidation_result={"passed": True},
        )
        self.assertFalse(decision.passed)
        self.assertIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)


class TestFailClosedMissingOrMalformedSymbolInputs(unittest.TestCase):
    """B10: missing symbol-inputs artifact file, or a malformed/wrong-schema
    artifact, fails premarket closed and blocks promotion."""

    def test_missing_symbol_inputs_file_raises_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            missing_path = tmp_root / "out" / "does_not_exist.json"
            with self.assertRaises((OSError, ValueError)):
                load_symbol_inputs_artifact(str(missing_path))

    def test_malformed_symbol_inputs_artifact_extracts_empty_map_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            watchlist = _make_ranked_watchlist()
            fit = _make_passing_strategy_fit()

            malformed_artifact = {"schema_version": "not-symbol-inputs-v1", "symbols": {"AAPL": {}}}
            self.assertNotEqual(malformed_artifact["schema_version"], SYMBOL_INPUTS_SCHEMA_VERSION)

            symbol_inputs_map = extract_symbol_inputs_map(malformed_artifact)
            self.assertEqual(symbol_inputs_map, {})

            premarket_result = evaluate_premarket_watchlist(
                watchlist, symbol_inputs_map, _passing_premarket_config(), reference_utc=_NOW
            )
            self.assertFalse(premarket_result.passed)
            self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, premarket_result.failure_reasons)

            decision = evaluate_watchlist_promotion(
                watchlist,
                {_SYMBOL: fit},
                _operator_approved_promotion_config(premarket_required=True),
                risk_simulation_result={"passed": True},
                premarket_revalidation_result=premarket_result.to_dict(),
            )
            self.assertFalse(decision.passed)
            self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)
            _ = tmp_root  # tmp dir kept alive for symmetry with other chain tests


if __name__ == "__main__":
    unittest.main()
