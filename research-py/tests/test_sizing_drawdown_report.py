from __future__ import annotations

import inspect
from pathlib import Path

import pandas as pd
import pytest

from mqk_research.reporting import sizing_drawdown_report as sdr
from mqk_research.reporting.sizing_drawdown_report import (
    SizingReportError,
    SizingReportSpec,
    TradeRecord,
    compute_sizing_drawdown_report,
    write_sizing_drawdown_report,
)


def _curve(values, start="2026-01-01"):
    ts = pd.date_range(start=start, periods=len(values), freq="D", tz="UTC")
    return pd.DataFrame({"ts": ts, "equity": values})


def _trade(**over):
    base = dict(
        trade_id="t1",
        symbol="AAA",
        side="long",
        qty=10.0,
        unit="shares",
        entry_ts="2026-01-01T00:00:00Z",
        entry_price=100.0,
        currency="USD",
        multiplier=1.0,
        costs_usd=0.0,
    )
    base.update(over)
    return TradeRecord(**base)


# ---------------------------------------------------------------------------
# Exact bps interpretation (frozen M1 reference values)
# ---------------------------------------------------------------------------

def test_exact_1000_bps_is_ten_percent_of_initial_capital():
    spec = SizingReportSpec(initial_strategy_capital_usd=100_000.0, capital_fraction_bps=1000).normalized()
    assert spec.capital_fraction == pytest.approx(0.10)
    assert spec.strategy_budget() == pytest.approx(10_000.0)


def test_exact_500_bps_stress_is_five_percent_of_initial_capital():
    spec = SizingReportSpec(initial_strategy_capital_usd=100_000.0, capital_fraction_bps=500).normalized()
    assert spec.capital_fraction == pytest.approx(0.05)
    assert spec.strategy_budget() == pytest.approx(5_000.0)


def test_capital_fraction_bps_out_of_range_fails_closed():
    with pytest.raises(SizingReportError):
        SizingReportSpec(capital_fraction_bps=10_001).normalized()
    with pytest.raises(SizingReportError):
        SizingReportSpec(capital_fraction_bps=-1).normalized()


# ---------------------------------------------------------------------------
# Unsupported unit/currency/multiplier/quantity -> fail closed, no silent conversion
# ---------------------------------------------------------------------------

def test_unsupported_quantity_unit_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="unit"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(unit="contracts")])


def test_unsupported_currency_fails_closed_no_conversion():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="currency"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(currency="EUR")])


def test_unsupported_multiplier_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="multiplier"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(multiplier=100.0)])


def test_missing_entry_price_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="entry_price"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(entry_price=0.0)])


def test_exit_without_exit_price_fails_closed():
    eq = _curve([100_000, 100_000])
    bad = TradeRecord(
        trade_id="t1", symbol="AAA", side="long", qty=10.0, unit="shares",
        entry_ts="2026-01-01T00:00:00Z", entry_price=100.0, currency="USD",
        multiplier=1.0, costs_usd=0.0, exit_ts="2026-01-02T00:00:00Z", exit_price=None,
    )
    with pytest.raises(SizingReportError, match="exit_price"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[bad])


def test_strategy_equity_curve_missing_columns_fails_closed():
    bad = pd.DataFrame({"ts": ["2026-01-01"], "nav": [100.0]})
    with pytest.raises(SizingReportError, match="equity curve"):
        compute_sizing_drawdown_report(strategy_equity=bad, account_equity=None, trades=[])


# ---------------------------------------------------------------------------
# Zero allocation / zero trades — no fabricated numbers
# ---------------------------------------------------------------------------

def test_zero_allocation_turnover_is_not_evaluable_not_a_zero_division():
    eq = _curve([100_000, 100_000])
    spec = SizingReportSpec(capital_fraction_bps=0)
    report = compute_sizing_drawdown_report(
        strategy_equity=eq, account_equity=None, trades=[_trade()], spec=spec
    )
    assert report["strategy_budget_usd"] == 0.0
    assert report["trades"]["turnover_vs_strategy_budget"] is None


def test_zero_trades_report_has_no_fabricated_pnl():
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["trades"]["count"] == 0
    assert report["trades"]["realized_gross_pnl_usd_total"] is None
    assert report["trades"]["realized_net_pnl_usd_total"] is None
    assert report["trades"]["unrealized_pnl_usd"] == 0.0  # no open positions either
    assert report["strategy"]["has_trades"] is False


def test_open_position_with_no_mark_is_unrealized_not_evaluable_not_fabricated_zero():
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade()])
    assert report["trades"]["open_count"] == 1
    assert report["trades"]["unrealized_pnl_usd"] == {
        "truth_state": "not_evaluable",
        "reason": "no mark price supplied for open positions",
    }


# ---------------------------------------------------------------------------
# Losing trades / costs exceeding gross profit / gap losses
# ---------------------------------------------------------------------------

def test_losing_trade_reports_negative_realized_pnl():
    eq = _curve([100_000, 100_000])
    t = _trade(entry_price=100.0, exit_ts="2026-01-02T00:00:00Z", exit_price=90.0, qty=10.0)
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])
    assert report["trades"]["realized_gross_pnl_usd_total"] == pytest.approx(-100.0)
    assert report["trades"]["realized_net_pnl_usd_total"] == pytest.approx(-100.0)


def test_short_side_pnl_direction_is_inverted():
    eq = _curve([100_000, 100_000])
    t = _trade(side="short", entry_price=100.0, exit_ts="2026-01-02T00:00:00Z", exit_price=90.0, qty=10.0)
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])
    # price fell 10/share; a short gains, not loses
    assert report["trades"]["realized_gross_pnl_usd_total"] == pytest.approx(100.0)


def test_costs_larger_than_gross_profit_is_flagged():
    eq = _curve([100_000, 100_000])
    t = _trade(entry_price=100.0, exit_ts="2026-01-02T00:00:00Z", exit_price=105.0, qty=10.0, costs_usd=60.0)
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])
    assert report["trades"]["realized_gross_pnl_usd_total"] == pytest.approx(50.0)
    assert report["trades"]["realized_net_pnl_usd_total"] == pytest.approx(-10.0)
    assert report["trades"]["costs_exceed_gross_profit"] is True


def test_gap_loss_is_captured_by_max_drawdown():
    # peak 120 on day1, gap down to 90 on day2 (-25%), partial recovery to 110 on day3
    eq = _curve([100, 120, 90, 110])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["equity_metrics"]["max_drawdown"] == pytest.approx(-0.25)


def test_max_drawdown_matches_a_hand_computed_oracle():
    # peaks: 100,120,120,120 ; dd = curve/peak - 1 = 0, 0, -0.25, -0.0833...
    eq = _curve([100, 120, 90, 110])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["equity_metrics"]["max_drawdown"] == pytest.approx(min([0.0, 0.0, 90 / 120 - 1.0, 110 / 120 - 1.0]))


def test_time_underwater_still_underwater_at_series_end_is_not_assumed_recovered():
    # peak at day index 1 (value 120); never returns to >=120 by day 3
    eq = _curve([100, 120, 90, 110])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["time_underwater_seconds"] == pytest.approx(2 * 86400.0)


# ---------------------------------------------------------------------------
# Partial fills aggregate into one logical position
# ---------------------------------------------------------------------------

def test_partial_fills_aggregate_into_one_logical_position_not_double_counted():
    t1 = _trade(trade_id="A1", qty=5.0, entry_price=50.0)
    t2 = _trade(trade_id="A2", partial_fill_of="A1", qty=5.0, entry_price=52.0)
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t1, t2])
    assert report["trades"]["count"] == 2
    assert report["trades"]["logical_position_count"] == 1
    # total notional traded still counts both fills (real cash moved both times)
    assert report["trades"]["total_notional_traded_usd"] == pytest.approx(5 * 50.0 + 5 * 52.0)


# ---------------------------------------------------------------------------
# Portfolio concentration / concurrency
# ---------------------------------------------------------------------------

def test_concentration_measured_at_peak_concurrency_not_trivial_solo_moments():
    # t1 alone for a day (trivially 100% concentrated), then t2 opens
    # alongside it (2 concurrent, 1000 vs 3000 -> 75% concentration at the
    # peak-concurrency moment), then both close.
    t1 = _trade(trade_id="t1", qty=10.0, entry_price=100.0,
                entry_ts="2026-01-01T00:00:00Z", exit_ts="2026-01-03T00:00:00Z", exit_price=100.0)
    t2 = _trade(trade_id="t2", qty=30.0, entry_price=100.0,
                entry_ts="2026-01-02T00:00:00Z", exit_ts="2026-01-03T00:00:00Z", exit_price=100.0)
    eq = _curve([100_000, 100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t1, t2])
    assert report["trades"]["max_concurrent_positions"] == 2
    assert report["trades"]["concentration_pct_at_max_concurrency"] == pytest.approx(3000.0 / 4000.0)


def test_no_trades_has_no_concurrency_fabricated():
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["trades"]["max_concurrent_positions"] == 0
    assert report["trades"]["concentration_pct_at_max_concurrency"] is None


# ---------------------------------------------------------------------------
# Strategy-budget vs account-equity: must not leak / must not be conflated
# ---------------------------------------------------------------------------

def test_strategy_negative_pnl_is_not_hidden_by_a_positive_account_curve():
    strategy_eq = _curve([100_000, 80_000])  # strategy lost money
    account_eq = _curve([5_000_000, 5_200_000])  # whole account still up
    report = compute_sizing_drawdown_report(strategy_equity=strategy_eq, account_equity=account_eq, trades=[])
    assert report["strategy"]["equity_metrics"]["end_equity"] < report["strategy"]["equity_metrics"]["start_equity"]
    assert report["account"]["equity_metrics"]["end_equity"] > report["account"]["equity_metrics"]["start_equity"]
    assert report["strategy"]["equity_metrics"]["max_drawdown"] < report["account"]["equity_metrics"]["max_drawdown"]


def test_account_not_provided_is_explicit_not_fabricated():
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["account"] == {"truth_state": "not_provided"}


# ---------------------------------------------------------------------------
# Capital exhaustion / insolvency
# ---------------------------------------------------------------------------

def test_capital_exhaustion_sets_insolvent_flag():
    eq = _curve([100_000, 50_000, 0.0, -10.0])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["insolvent"] is True


def test_positive_curve_is_not_insolvent():
    eq = _curve([100_000, 90_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["insolvent"] is False


# ---------------------------------------------------------------------------
# Determinism / mutation proof / identity
# ---------------------------------------------------------------------------

def test_deterministic_report_regeneration_same_report_id():
    eq = _curve([100_000, 90_000, 110_000])
    t = _trade()
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t])
    assert r1["report_id"] == r2["report_id"]


def test_mutation_changed_trade_price_changes_report_id():
    eq = _curve([100_000, 90_000, 110_000])
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[_trade(entry_price=100.0)])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[_trade(entry_price=101.0)])
    assert r1["report_id"] != r2["report_id"]


def test_mutation_changed_capital_fraction_changes_report_id_and_budget():
    eq = _curve([100_000, 100_000])
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[],
                                         spec=SizingReportSpec(capital_fraction_bps=1000))
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[],
                                         spec=SizingReportSpec(capital_fraction_bps=500))
    assert r1["report_id"] != r2["report_id"]
    assert r1["strategy_budget_usd"] != r2["strategy_budget_usd"]


# ---------------------------------------------------------------------------
# File-driving wrapper round-trip
# ---------------------------------------------------------------------------

def test_write_sizing_drawdown_report_round_trip(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 95_000, 105_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([{
        "trade_id": "t1", "symbol": "AAA", "side": "long", "qty": 10.0,
        "entry_ts": "2026-01-01T00:00:00Z", "entry_price": 100.0,
        "exit_ts": "2026-01-02T00:00:00Z", "exit_price": 95.0,
        "costs_usd": 1.0,
    }]).to_csv(trades_path, index=False)
    out_path = tmp_path / "out" / "report.json"

    result_path = write_sizing_drawdown_report(
        strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
    )
    assert result_path == out_path
    assert out_path.exists()

    import json
    loaded = json.loads(out_path.read_text(encoding="utf-8"))
    assert loaded["schema_version"] == "sizing_drawdown_report_v1"
    assert loaded["trades"]["realized_gross_pnl_usd_total"] == pytest.approx(-50.0)
    assert loaded["trades"]["realized_net_pnl_usd_total"] == pytest.approx(-51.0)


# ---------------------------------------------------------------------------
# Negative control: this module must not touch any production sizing/
# execution/provider authority (no economic trial, no network, no native
# process invocation).
# ---------------------------------------------------------------------------

def test_module_does_not_import_subprocess_network_or_native_cli_paths():
    src = inspect.getsource(sdr)
    for forbidden in ("subprocess", "requests", "httpx", "socket", "mqk-cli", "mqk_cli"):
        assert forbidden not in src, f"unexpected production/network/native reference: {forbidden!r}"
