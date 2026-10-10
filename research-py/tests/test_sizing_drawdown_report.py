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

def _full_trade_row(**over):
    row = {
        "trade_id": "t1", "symbol": "AAA", "side": "long", "qty": 10.0,
        "unit": "shares", "entry_ts": "2026-01-01T00:00:00Z", "entry_price": 100.0,
        "currency": "USD", "multiplier": 1.0, "costs_usd": 1.0,
        "exit_ts": "2026-01-02T00:00:00Z", "exit_price": 95.0,
    }
    row.update(over)
    return row


def test_write_sizing_drawdown_report_round_trip(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 95_000, 105_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([_full_trade_row()]).to_csv(trades_path, index=False)
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
    assert loaded["input_evidence_hash"]["trades"] is not None
    assert loaded["input_evidence_hash"]["strategy_equity"] is not None


# ---------------------------------------------------------------------------
# B1: explicitly-supplied-but-missing/unreadable trades input is an error,
# never a silent zero-trade report. Omitting it entirely is the only valid
# way to declare zero trades.
# ---------------------------------------------------------------------------

def test_nonexistent_trades_csv_path_is_an_error_not_zero_trades(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="does not exist"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None,
            trades_csv=tmp_path / "nonexistent_trades.csv", out_json=out_path,
        )


def test_omitted_trades_csv_is_a_valid_explicit_zero_trades_declaration(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    out_path = tmp_path / "report.json"
    result_path = write_sizing_drawdown_report(
        strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=None, out_json=out_path,
    )
    import json
    loaded = json.loads(result_path.read_text(encoding="utf-8"))
    assert loaded["trades"]["count"] == 0


def test_empty_but_well_formed_trades_csv_is_a_valid_explicit_zero_trades_declaration(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame(columns=list(sdr._REQUIRED_TRADE_COLUMNS)).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    result_path = write_sizing_drawdown_report(
        strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
    )
    import json
    loaded = json.loads(result_path.read_text(encoding="utf-8"))
    assert loaded["trades"]["count"] == 0


def test_unreadable_trades_csv_fails_closed(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    trades_path.write_text("this is not,,, valid\ncsv\"\"\" structure {{{", encoding="utf-8")
    # Force a genuinely unparseable table (mismatched quoting breaks the C parser).
    trades_path.write_text('"trade_id,symbol\n"unterminated quote,AAA\n"', encoding="utf-8")
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="unreadable/malformed"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
        )


@pytest.mark.parametrize("missing_col", ["unit", "currency", "multiplier", "costs_usd"])
def test_missing_required_trade_column_fails_closed(tmp_path: Path, missing_col):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    row = _full_trade_row()
    del row[missing_col]
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([row]).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="missing required columns"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
        )


@pytest.mark.parametrize("blank_col", ["unit", "currency", "multiplier", "costs_usd", "qty", "entry_price"])
def test_present_but_blank_trade_value_fails_closed(tmp_path: Path, blank_col):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    row = _full_trade_row()
    row[blank_col] = None  # column exists, value is blank/missing for this row
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([row]).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="missing value|malformed trade data"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
        )


def test_malformed_numeric_value_in_trade_row_fails_closed(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    row = _full_trade_row(qty="not_a_number")
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([row]).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="malformed trade data"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
        )


def test_explicit_zero_cost_is_preserved_not_treated_as_missing(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([_full_trade_row(costs_usd=0.0)]).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    result_path = write_sizing_drawdown_report(
        strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
    )
    import json
    loaded = json.loads(result_path.read_text(encoding="utf-8"))
    assert loaded["trades"]["costs_usd_total"] == 0.0


def test_missing_strategy_equity_csv_fails_closed(tmp_path: Path):
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="does not exist"):
        write_sizing_drawdown_report(
            strategy_equity_csv=tmp_path / "nonexistent.csv", account_equity_csv=None,
            trades_csv=None, out_json=out_path,
        )


# ---------------------------------------------------------------------------
# B3: partial fills must not inflate concurrency; staggered legs of one
# logical position collapse to one concurrently-open position.
# ---------------------------------------------------------------------------

def test_two_partial_fills_of_one_position_plus_one_independent_position_counts_two_not_three():
    fill_a = _trade(trade_id="A1", qty=5.0, entry_price=50.0,
                     entry_ts="2026-01-01T00:00:00Z", exit_ts="2026-01-05T00:00:00Z", exit_price=55.0)
    fill_b = _trade(trade_id="A2", partial_fill_of="A1", qty=5.0, entry_price=52.0,
                     entry_ts="2026-01-02T00:00:00Z", exit_ts="2026-01-05T00:00:00Z", exit_price=55.0)
    independent = _trade(trade_id="B1", qty=20.0, entry_price=10.0,
                          entry_ts="2026-01-03T00:00:00Z", exit_ts="2026-01-04T00:00:00Z", exit_price=11.0)
    eq = _curve([100_000, 100_000, 100_000, 100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[fill_a, fill_b, independent])
    # Old buggy behavior would have reported 3 (one per fill row); the
    # correct logical count is 2: {A1-group, B1}.
    assert report["trades"]["max_concurrent_positions"] == 2


def test_partial_fill_group_stays_open_until_every_leg_is_closed():
    fill_a = _trade(trade_id="A1", qty=5.0, entry_price=50.0,
                     entry_ts="2026-01-01T00:00:00Z", exit_ts="2026-01-03T00:00:00Z", exit_price=55.0)
    fill_b_still_open = _trade(trade_id="A2", partial_fill_of="A1", qty=5.0, entry_price=52.0,
                                entry_ts="2026-01-02T00:00:00Z")  # no exit at all
    other = _trade(trade_id="B1", qty=20.0, entry_price=10.0,
                    entry_ts="2026-01-04T00:00:00Z")  # opens after A1's one leg closed
    eq = _curve([100_000, 100_000, 100_000, 100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[fill_a, fill_b_still_open, other])
    # A1-group never fully closes (A2 leg still open), so it is still open
    # when B1 opens on day 4 -> 2 concurrent, not treated as sequential.
    assert report["trades"]["max_concurrent_positions"] == 2


# ---------------------------------------------------------------------------
# B4: time underwater must count through the actual recovery instant, not
# just the last strictly-underwater sample before it.
# ---------------------------------------------------------------------------

def test_time_underwater_counts_through_the_recovery_instant_mid_series():
    # peak=120 on day1, drops to 50 on day2 (1 day underwater by the old
    # buggy measurement), recovers to exactly 120 on day3. True episode
    # length is 2 days (day1 -> day3), not 1.
    eq = _curve([100, 120, 50, 120])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["time_underwater_seconds"] == pytest.approx(2 * 86400.0)


def test_flat_curve_has_zero_time_underwater_no_false_positive():
    eq = _curve([100, 100, 100])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["time_underwater_seconds"] == pytest.approx(0.0)


def test_repeated_peaks_do_not_manufacture_underwater_time():
    eq = _curve([100, 120, 120, 120])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["time_underwater_seconds"] == pytest.approx(0.0)


def test_delayed_recovery_measures_the_full_episode():
    # drop on day1, stays down through day4, recovers exactly on day5.
    eq = _curve([100, 50, 50, 50, 100])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["time_underwater_seconds"] == pytest.approx(4 * 86400.0)


# ---------------------------------------------------------------------------
# B5: comprehensive numeric/timestamp validation
# ---------------------------------------------------------------------------

def test_nan_capital_fails_closed():
    with pytest.raises(SizingReportError, match="finite"):
        SizingReportSpec(initial_strategy_capital_usd=float("nan")).normalized()


def test_infinite_capital_fails_closed():
    with pytest.raises(SizingReportError, match="finite"):
        SizingReportSpec(initial_strategy_capital_usd=float("inf")).normalized()


def test_fractional_bps_fails_closed():
    with pytest.raises(SizingReportError, match="integer"):
        SizingReportSpec(capital_fraction_bps=999.5).normalized()


def test_bool_as_bps_fails_closed():
    with pytest.raises(SizingReportError, match="bool"):
        SizingReportSpec(capital_fraction_bps=True).normalized()


def test_nan_bps_fails_closed():
    with pytest.raises(SizingReportError, match="finite"):
        SizingReportSpec(capital_fraction_bps=float("nan")).normalized()


def test_zero_trading_days_per_year_fails_closed():
    with pytest.raises(SizingReportError, match="positive integer"):
        SizingReportSpec(trading_days_per_year=0).normalized()


def test_negative_trading_days_per_year_fails_closed():
    with pytest.raises(SizingReportError, match="positive integer"):
        SizingReportSpec(trading_days_per_year=-252).normalized()


def test_nan_qty_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="finite"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(qty=float("nan"))])


def test_infinite_entry_price_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="finite"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(entry_price=float("inf"))])


def test_nan_costs_fails_closed():
    eq = _curve([100_000, 100_000])
    with pytest.raises(SizingReportError, match="finite"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(costs_usd=float("nan"))])


def test_exit_before_entry_fails_closed():
    eq = _curve([100_000, 100_000])
    t = _trade(entry_ts="2026-01-05T00:00:00Z", exit_ts="2026-01-01T00:00:00Z", exit_price=100.0)
    with pytest.raises(SizingReportError, match="before entry_ts"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])


def test_malformed_entry_ts_fails_closed():
    eq = _curve([100_000, 100_000])
    t = _trade(entry_ts="not-a-timestamp")
    with pytest.raises(SizingReportError, match="not a parseable timestamp"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])


def test_empty_equity_curve_fails_closed():
    eq = pd.DataFrame(columns=["ts", "equity"])
    with pytest.raises(SizingReportError, match="zero rows"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])


def test_nan_in_equity_curve_fails_closed():
    eq = _curve([100_000, float("nan"), 100_000])
    with pytest.raises(SizingReportError, match="NaN/Infinity"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])


def test_legitimate_undefined_sharpe_is_not_evaluable_not_nan_not_zero():
    # A single-point curve: CAGR/Sharpe are genuinely undefined.
    eq = _curve([100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["strategy"]["equity_metrics"]["sharpe"] == {
        "truth_state": "not_evaluable",
        "reason": "undefined for this equity curve (insufficient history, zero variance, or non-positive equity)",
    }
    # And the final JSON must contain no raw NaN/Infinity tokens.
    import json
    text = json.dumps(report, allow_nan=False)  # must not raise
    assert "NaN" not in text and "Infinity" not in text


# ---------------------------------------------------------------------------
# B6: report identity must be bound to the raw input evidence, not just
# summarized output aggregates.
# ---------------------------------------------------------------------------

def test_different_trade_histories_with_identical_aggregates_get_different_report_ids():
    eq = _curve([100_000, 100_000])
    # Two trades with the same net PnL (+50 - 10 = +40) via completely
    # different underlying prices/quantities -- aggregates coincide.
    t1 = _trade(trade_id="x1", qty=10.0, entry_price=100.0, exit_ts="2026-01-02T00:00:00Z", exit_price=105.0, costs_usd=10.0)
    t2 = _trade(trade_id="x2", qty=5.0, entry_price=50.0, exit_ts="2026-01-02T00:00:00Z", exit_price=60.0, costs_usd=10.0)
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t1])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t2])
    assert r1["trades"]["realized_net_pnl_usd_total"] == pytest.approx(r2["trades"]["realized_net_pnl_usd_total"])
    assert r1["report_id"] != r2["report_id"]
    assert r1["input_evidence_hash"]["trades"] != r2["input_evidence_hash"]["trades"]


def test_mutating_only_an_untracked_instrument_field_still_changes_report_id():
    eq = _curve([100_000, 100_000])
    t1 = _trade(trade_id="x1", symbol="AAA")
    t2 = _trade(trade_id="x1", symbol="BBB")  # same aggregate-relevant fields, different symbol
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t1])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t2])
    assert r1["report_id"] != r2["report_id"]


# ---------------------------------------------------------------------------
# Negative control: this module must not touch any production sizing/
# execution/provider authority (no economic trial, no network, no native
# process invocation).
# ---------------------------------------------------------------------------

def test_module_does_not_import_subprocess_network_or_native_cli_paths():
    src = inspect.getsource(sdr)
    for forbidden in ("subprocess", "requests", "httpx", "socket", "mqk-cli", "mqk_cli"):
        assert forbidden not in src, f"unexpected production/network/native reference: {forbidden!r}"


# ---------------------------------------------------------------------------
# FW-B3-R2 (surgical closeout 02): causal per-timestamp notional
# accumulation. The PREVIOUS fix grouped fills into logical positions
# correctly, but then summed ALL of a group's fills' notional (including
# fills that haven't happened yet) and backdated the total to the group's
# EARLIEST fill. Exact oracle from the review: logical A fills $100 Jan1
# (never closes) and $900 Jan10 (never closes); logical B fills $100
# Jan2, closes Jan3. At peak concurrency (2, during Jan2-Jan3), the TRUE
# entry-notional concentration is 0.50 (A=$100 vs B=$100 at that time);
# the previous bug reported 0.90909 (it had already backdated A's future
# $900 add to Jan1).
# ---------------------------------------------------------------------------

def test_b3_oracle_exact_fixed_expected_value_not_just_result_to_result():
    a_fill1 = _trade(trade_id="A1", qty=1.0, entry_price=100.0, entry_ts="2026-01-01T00:00:00Z")
    a_fill2 = _trade(trade_id="A2", partial_fill_of="A1", qty=1.0, entry_price=900.0, entry_ts="2026-01-10T00:00:00Z")
    b_fill = _trade(trade_id="B1", qty=1.0, entry_price=100.0, entry_ts="2026-01-02T00:00:00Z",
                     exit_ts="2026-01-03T00:00:00Z", exit_price=100.0)
    eq = _curve([100_000] * 10, start="2026-01-01")
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[a_fill1, a_fill2, b_fill])
    assert report["trades"]["max_concurrent_positions"] == 2
    # Fixed literal expected value -- not a result-to-result comparison.
    assert report["trades"]["concentration_pct_at_max_concurrency"] == pytest.approx(0.50)
    # Negative assertion against the PREVIOUS (buggy) value, so a mutant
    # that reintroduces the backdating bug is caught even if 0.50 were
    # coincidentally produced some other wrong way.
    assert report["trades"]["concentration_pct_at_max_concurrency"] != pytest.approx(0.90909, abs=1e-4)


def test_b3_partial_close_before_a_later_add_stays_open_at_reduced_notional():
    # fill1 (group A1) opens Jan1 notional=500, closes Jan5.
    # fill2 (group A1, partial_fill_of) opens Jan3 notional=300, closes Jan10.
    # At Jan5 (fill1 closes), the group must NOT fully close -- fill2 is
    # still open, so the position's running notional drops to 300, not 0.
    fill1 = _trade(trade_id="A1", qty=5.0, entry_price=100.0, entry_ts="2026-01-01T00:00:00Z",
                    exit_ts="2026-01-05T00:00:00Z", exit_price=100.0)
    fill2 = _trade(trade_id="A2", partial_fill_of="A1", qty=3.0, entry_price=100.0, entry_ts="2026-01-03T00:00:00Z",
                    exit_ts="2026-01-10T00:00:00Z", exit_price=100.0)
    other = _trade(trade_id="B1", qty=1.0, entry_price=1000.0, entry_ts="2026-01-06T00:00:00Z")  # still open
    eq = _curve([100_000] * 10, start="2026-01-01")
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[fill1, fill2, other])
    # Group A1 stays open past Jan5 (fill2 still active) and is concurrent
    # with B1 (opens Jan6) -> 2 concurrent positions, not sequential.
    assert report["trades"]["max_concurrent_positions"] == 2


def test_b3_simultaneous_open_and_close_at_the_same_instant_is_deterministic():
    closing = _trade(trade_id="X1", qty=1.0, entry_price=100.0, entry_ts="2026-01-01T00:00:00Z",
                      exit_ts="2026-01-05T00:00:00Z", exit_price=100.0)
    opening = _trade(trade_id="X2", qty=1.0, entry_price=100.0, entry_ts="2026-01-05T00:00:00Z")  # same instant
    eq = _curve([100_000] * 10, start="2026-01-01")
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[closing, opening])
    # Close-before-open convention at a tie: never both counted open at once.
    assert report["trades"]["max_concurrent_positions"] == 1


def test_b3_missing_close_remains_active_through_the_whole_series():
    never_closes = _trade(trade_id="Y1", qty=1.0, entry_price=100.0, entry_ts="2026-01-01T00:00:00Z")
    eq = _curve([100_000, 100_000], start="2026-01-01")
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[never_closes])
    assert report["trades"]["max_concurrent_positions"] == 1


# ---------------------------------------------------------------------------
# FW-B5-R3 (surgical closeout 02): compounding must be a real bool.
# bool("false") is True in Python -- a blind bool(...) coercion would have
# silently enabled compounding for the STRING "false".
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("bad_value", ["false", "true", 0, 1, None, "0"])
def test_compounding_non_bool_fails_closed(bad_value):
    with pytest.raises(SizingReportError, match="compounding must be a real bool"):
        SizingReportSpec(compounding=bad_value).normalized()


def test_compounding_real_bool_true_and_false_both_still_work():
    assert SizingReportSpec(compounding=True).normalized().compounding is True
    assert SizingReportSpec(compounding=False).normalized().compounding is False


# ---------------------------------------------------------------------------
# FW-B5-R4 (surgical closeout 02): None means not supplied; an explicitly
# supplied but EMPTY account_equity must fail closed, not be silently
# represented as not_provided.
# ---------------------------------------------------------------------------

def test_account_equity_none_is_genuinely_not_provided():
    eq = _curve([100_000, 100_000])
    report = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
    assert report["account"] == {"truth_state": "not_provided"}


def test_account_equity_explicitly_empty_dataframe_fails_closed_not_not_provided():
    eq = _curve([100_000, 100_000])
    empty_account = pd.DataFrame(columns=["ts", "equity"])
    with pytest.raises(SizingReportError, match="zero rows"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=empty_account, trades=[])


# ---------------------------------------------------------------------------
# Addendum: exit_price without exit_ts must be rejected (symmetric to the
# already-existing exit_ts-without-exit_price check); empty trade_id/symbol
# rejected; duplicate trade_id rejected; orphan/inconsistent partial-fill
# groups rejected.
# ---------------------------------------------------------------------------

def test_exit_price_without_exit_ts_fails_closed():
    t = _trade(exit_ts=None, exit_price=120.0)
    with pytest.raises(SizingReportError, match="exit_price present but exit_ts missing"):
        t.validate()


def test_open_trade_with_neither_exit_field_remains_valid():
    t = _trade(exit_ts=None, exit_price=None)
    t.validate()  # must not raise
    assert t.is_closed is False


def test_closed_trade_with_both_exit_fields_remains_valid():
    t = _trade(exit_ts="2026-01-02T00:00:00Z", exit_price=120.0)
    t.validate()  # must not raise
    assert t.is_closed is True


def test_empty_trade_id_fails_closed():
    t = _trade(trade_id="")
    with pytest.raises(SizingReportError, match="trade_id must be a non-empty string"):
        t.validate()


def test_whitespace_only_trade_id_fails_closed():
    t = _trade(trade_id="   ")
    with pytest.raises(SizingReportError, match="trade_id must be a non-empty string"):
        t.validate()


def test_empty_symbol_fails_closed():
    t = _trade(symbol="")
    with pytest.raises(SizingReportError, match="symbol must be a non-empty string"):
        t.validate()


def test_duplicate_trade_id_across_distinct_trades_fails_closed():
    eq = _curve([100_000, 100_000])
    t1 = _trade(trade_id="dup", symbol="AAA")
    t2 = _trade(trade_id="dup", symbol="BBB")
    with pytest.raises(SizingReportError, match="duplicate trade_id"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t1, t2])


def test_orphan_partial_fill_of_reference_fails_closed():
    eq = _curve([100_000, 100_000])
    t = _trade(trade_id="A2", partial_fill_of="NONEXISTENT_BASE")
    with pytest.raises(SizingReportError, match="orphan group"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t])


def test_inconsistent_symbol_within_a_partial_fill_group_fails_closed():
    eq = _curve([100_000, 100_000])
    t1 = _trade(trade_id="A1", symbol="AAA")
    t2 = _trade(trade_id="A2", partial_fill_of="A1", symbol="BBB")
    with pytest.raises(SizingReportError, match="inconsistent symbol"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t1, t2])


def test_inconsistent_side_within_a_partial_fill_group_fails_closed():
    eq = _curve([100_000, 100_000])
    t1 = _trade(trade_id="A1", side="long")
    t2 = _trade(trade_id="A2", partial_fill_of="A1", side="short")
    with pytest.raises(SizingReportError, match="inconsistent side"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[t1, t2])


# ---------------------------------------------------------------------------
# Addendum: naive timestamps and mixed naive/aware comparisons must fail
# closed with SizingReportError, never an unrelated raw TypeError.
# ---------------------------------------------------------------------------

def test_naive_entry_ts_fails_closed():
    t = _trade(entry_ts="2026-01-01")  # no time/tz component
    with pytest.raises(SizingReportError, match="timezone-aware"):
        t.validate()


def test_naive_exit_ts_with_aware_entry_fails_closed_not_raw_type_error():
    t = _trade(entry_ts="2026-01-01T00:00:00Z", exit_ts="2026-01-02", exit_price=100.0)
    with pytest.raises(SizingReportError, match="timezone-aware"):
        t.validate()


def test_naive_entry_ts_with_aware_exit_fails_closed_not_raw_type_error():
    # This exact combination previously reached the exit<entry comparison
    # with one naive and one aware Timestamp, raising an uncontrolled
    # `TypeError: Cannot compare tz-naive and tz-aware timestamps`.
    t = _trade(entry_ts="2026-01-01", exit_ts="2026-01-02T00:00:00Z", exit_price=100.0)
    with pytest.raises(SizingReportError, match="timezone-aware"):
        t.validate()


# ---------------------------------------------------------------------------
# Addendum: equity-curve timestamp validity/uniqueness, and a canonical,
# row-order-independent content hash.
# ---------------------------------------------------------------------------

def test_equity_curve_malformed_timestamp_fails_closed_with_sizing_report_error():
    eq = pd.DataFrame({"ts": ["not-a-timestamp"], "equity": [100_000.0]})
    with pytest.raises(SizingReportError, match="unparseable timestamp"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])


def test_equity_curve_naive_timestamp_fails_closed():
    eq = pd.DataFrame({"ts": ["2026-01-01T00:00:00"], "equity": [100_000.0]})  # no tz
    with pytest.raises(SizingReportError, match="timezone-aware"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])


def test_equity_curve_duplicate_timestamps_fails_closed():
    eq = pd.DataFrame({
        "ts": ["2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        "equity": [100_000.0, 99_000.0],
    })
    with pytest.raises(SizingReportError, match="duplicate timestamps"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])


def test_equity_curve_content_hash_is_independent_of_row_order():
    eq_sorted = _curve([100_000, 95_000, 105_000])
    eq_shuffled = eq_sorted.iloc[[2, 0, 1]].reset_index(drop=True)  # same rows, different order
    r1 = compute_sizing_drawdown_report(strategy_equity=eq_sorted.copy(), account_equity=None, trades=[])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq_shuffled.copy(), account_equity=None, trades=[])
    assert r1["input_evidence_hash"]["strategy_equity"] == r2["input_evidence_hash"]["strategy_equity"]


def test_equity_curve_content_hash_changes_on_a_real_value_change():
    eq1 = _curve([100_000, 95_000])
    eq2 = _curve([100_000, 95_001])
    r1 = compute_sizing_drawdown_report(strategy_equity=eq1, account_equity=None, trades=[])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq2, account_equity=None, trades=[])
    assert r1["input_evidence_hash"]["strategy_equity"] != r2["input_evidence_hash"]["strategy_equity"]


# ---------------------------------------------------------------------------
# Second-sweep negatives: _trade_content_hash row-layout independence, and
# duplicate trade_id detection through the actual CSV loading path (not
# just the in-memory API).
# ---------------------------------------------------------------------------

def test_trade_content_hash_is_independent_of_trade_list_order():
    eq = _curve([100_000, 100_000])
    t1 = _trade(trade_id="t1", entry_price=100.0)
    t2 = _trade(trade_id="t2", entry_price=200.0)
    r1 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t1, t2])
    r2 = compute_sizing_drawdown_report(strategy_equity=eq.copy(), account_equity=None, trades=[t2, t1])
    assert r1["input_evidence_hash"]["trades"] == r2["input_evidence_hash"]["trades"]
    assert r1["report_id"] == r2["report_id"]


def test_duplicate_trade_id_via_csv_loading_path_fails_closed(tmp_path: Path):
    eq_path = tmp_path / "strategy_equity.csv"
    _curve([100_000, 100_000]).to_csv(eq_path, index=False)
    trades_path = tmp_path / "trades.csv"
    pd.DataFrame([
        _full_trade_row(trade_id="dup", symbol="AAA"),
        _full_trade_row(trade_id="dup", symbol="BBB"),
    ]).to_csv(trades_path, index=False)
    out_path = tmp_path / "report.json"
    with pytest.raises(SizingReportError, match="duplicate trade_id"):
        write_sizing_drawdown_report(
            strategy_equity_csv=eq_path, account_equity_csv=None, trades_csv=trades_path, out_json=out_path,
        )


# Independent-review completion: UTC, finite evidence, and active-fill accounting.
def test_ir_underwater_sorts_by_utc_instant_not_local_timestamp_text():
    eq = pd.DataFrame({
        "ts": ["2026-01-01T10:00:00-05:00", "2026-01-01T11:00:00Z", "2026-01-01T12:00:00Z"],
        "equity": [110.0, 100.0, 90.0],
    })
    assert sdr._time_underwater_seconds(eq) == 4 * 3600.0
    normalized = eq.assign(ts=pd.to_datetime(eq["ts"], utc=True))
    assert sdr._time_underwater_seconds(normalized) == 4 * 3600.0


def test_ir_equivalent_timezone_offsets_do_not_change_economic_evidence_identity():
    curve_utc = pd.DataFrame({"ts": ["2026-01-01T11:00:00Z", "2026-01-01T12:00:00Z"], "equity": [100., 90.]})
    curve_offset = pd.DataFrame({"ts": ["2026-01-01T12:00:00+01:00", "2026-01-01T13:00:00+01:00"], "equity": [100., 90.]})
    r1 = compute_sizing_drawdown_report(strategy_equity=curve_utc, account_equity=None, trades=[])
    r2 = compute_sizing_drawdown_report(strategy_equity=curve_offset, account_equity=None, trades=[])
    assert r1["input_evidence_hash"]["strategy_equity"] == r2["input_evidence_hash"]["strategy_equity"]
    assert r1["report_id"] == r2["report_id"]


def test_ir_derived_notional_overflow_must_not_serialize_infinity():
    eq = _curve([100., 90.])
    with pytest.raises(SizingReportError, match="non-finite|overflow|finite positive"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[_trade(qty=1e308, entry_price=100.)])


def test_ir_aggregate_of_individually_finite_values_cannot_overflow_report():
    eq = _curve([100., 90.])
    a = _trade(trade_id="A1", qty=1e308, entry_price=1.0)
    b = _trade(trade_id="B1", qty=1e308, entry_price=1.0)
    with pytest.raises(SizingReportError, match="non-finite|overflow|finite positive"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[a, b])


def test_ir_tiny_positive_notional_still_counts_as_open_position():
    eq = _curve([100., 90.])
    rep = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None,
        trades=[_trade(qty=1e-12, entry_price=100.)])
    assert rep["trades"]["max_concurrent_positions"] == 1
    assert rep["trades"]["concentration_pct_at_max_concurrency"] == 1.0


def test_ir_zero_duration_fill_cannot_remain_open_after_batched_events():
    eq = _curve([100., 90.])
    rep = compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None,
        trades=[_trade(entry_ts="2026-01-01T00:00:00Z", exit_ts="2026-01-01T00:00:00Z", exit_price=100.)])
    assert rep["trades"]["max_concurrent_positions"] == 0


def test_ir_cyclic_partial_fill_group_reference_is_rejected():
    eq = _curve([100., 90.])
    a = _trade(trade_id="A", partial_fill_of="B")
    b = _trade(trade_id="B", partial_fill_of="A")
    with pytest.raises(SizingReportError, match="root fill"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[a, b])


def test_ir_nested_partial_fill_chain_is_rejected_not_double_counted():
    eq = _curve([100., 90.])
    root = _trade(trade_id="A")
    child = _trade(trade_id="B", partial_fill_of="A")
    grandchild = _trade(trade_id="C", partial_fill_of="B")
    with pytest.raises(SizingReportError, match="root fill"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[root, child, grandchild])


def test_ir_true_is_not_a_valid_unit_multiplier():
    with pytest.raises(SizingReportError, match="multiplier"):
        _trade(multiplier=True).validate()


@pytest.mark.parametrize("field,value", [("trade_id", 123), ("symbol", 456)])
def test_ir_non_string_trade_identifiers_are_controlled_refusals(field, value):
    with pytest.raises(SizingReportError, match="non-empty string"):
        _trade(**{field: value}).validate()


def test_ir_invalid_equity_numeric_type_raises_report_specific_error():
    eq = pd.DataFrame({"ts": ["2026-01-01T00:00:00Z"], "equity": ["not-a-number"]})
    with pytest.raises(SizingReportError, match="non-numeric equity"):
        compute_sizing_drawdown_report(strategy_equity=eq, account_equity=None, trades=[])
