"""Pass-2 robustness purge invariants: protocol pins, result-independent identity, cohort rule, every Strategy/Conditional
scenario, discovery fence, durable attempts/resume, and label guards. Real-evidence tests run only when the accepted
Pass-1 run directory exists, and the real-engine ones only after the Pass-2 predeclaration exists (post-freeze)."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

TESTS = Path(__file__).resolve().parent
EXP2 = TESTS.parent / "experiments" / "alpha_edge_pass2_01"
EXP1 = TESTS.parent / "experiments" / "alpha_edge_census_01"
for _p in (str(TESTS), str(EXP2), str(EXP1)):
    sys.path.insert(0, _p)

import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import data as cdata  # noqa: E402
import edge_registry as er  # noqa: E402
import p2_conditional as p2c  # noqa: E402
import p2_cohort as pc  # noqa: E402
import p2_pass1 as p1  # noqa: E402
import p2_protocol as pp  # noqa: E402
import p2_runner as pr  # noqa: E402
import p2_strategy as p2s  # noqa: E402
import partitions as pt  # noqa: E402
import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
import simulate as sm  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from test_alpha_edge_census_01 import synth_bars  # noqa: E402

HAVE_RUN = p1.RUN_DIR.exists()
needs_run = pytest.mark.skipif(not HAVE_RUN, reason="accepted Pass-1 run directory not present")
needs_freeze = pytest.mark.skipif(not (HAVE_RUN and (EXP2 / "PASS2_PREDECLARATION.json").exists()),
                                  reason="Pass-2 predeclaration not yet committed (pre-freeze)")

PINNED_PROTOCOL_ID = "a712c301c2c527d8bc37b1ba7e72e6889c16a3ae0b2cd6e10f4dfbf2ee62db15"


# ------------------------------------------------------------------------------------------------- fixtures

@pytest.fixture(scope="module")
def bars():
    out = {"SPY": synth_bars("SPY", 1, phi=0.0, mu=0.0004, sig=0.009, price0=200.0)}
    for i in range(8):
        out[f"X{i:02d}"] = synth_bars(f"X{i:02d}", 100 + i, phi=-0.2 if i % 2 else 0.1)
    out["UP"] = synth_bars("UP", 7, phi=0.0, mu=0.0015, sig=0.006)
    return out


@pytest.fixture(scope="module")
def U(bars):
    return sg.Universe(bars)


@pytest.fixture(scope="module")
def meta(U):
    return {s: {"disposition": ce.DATA_PRESENT, "rows": U.sd[s].n, "data_short_history": False,
                "data_quality_caveat": False} for s in U.symbols}


def _cand(family, params, symbol, ci=0):
    return {"candidate_id": "c" * 32, "edge_id": "e" * 32, "trial_id": "t-1", "config_id": "cfg", "family": family,
            "params": params, "symbol": symbol, "config_index": ci}


def _ctx(U, meta, neighbors=None, moderate=None):
    return p2s.StrategyContext(U, meta, [], neighbors or [[]], moderate or set(), {0: 10}, {0: 3})


def _accepted(U, meta, family, params, symbol):
    return json.loads(json.dumps(ce.evaluate_cell(U, {"family": family, "params": params}, symbol, meta)["m"]))


# ----------------------------------------------------------------------------------------- protocol / identity

def test_protocol_thresholds_are_the_predeclared_numbers():
    assert (pp.STRESS_2X, pp.STRESS_3X, pp.DELAY_SESSIONS) == (2, 3, 1)
    assert pp.YEARS == tuple(range(2016, 2024)) and pp.MIN_POSITIVE_YEARS == 6 == -(-70 * 8 // 100)
    assert (pp.MIN_TRADES, pp.MAX_REGIME_CONCENTRATION, pp.MIN_NEIGHBOR_SHARE) == (30, 0.80, 0.25)
    assert (pp.INITIAL_CAPITAL_USD, pp.MAX_DRAWDOWN_FRAC, pp.ROLLING_SESSIONS, pp.MIN_ROLLING_RETURN) == (
        100_000.0, 0.20, 5, -0.06)
    assert (pp.C_MIN_EVENTS, pp.C_MAX_TOP_SYMBOL_SHARE, pp.C_MIN_NEIGHBOR_SHARE, pp.C_REGIME_CONCENTRATION) == (
        30, 0.50, 0.25, 0.80)
    assert pp.HORIZONS == ss.CONDITIONAL_HORIZONS
    assert pp.STRATEGY_HARD_GATES == ("S1", "S2", "S3", "S5", "S6", "S7", "S8", "S9", "S10")
    assert pp.CONDITIONAL_HARD_GATES == ("C1", "C2", "C3", "C4", "C5", "C6")
    assert pp.LABELS["VALIDATION_STATUS"] == "NOT_VALIDATED" and pp.LABELS["PROMOTION_AUTHORITY"] == "NONE"


def test_protocol_document_binds_the_thresholds_into_the_protocol_id():
    base = pp.protocol_id()
    assert base == pp.PASS2_PROTOCOL_ID
    for path, val in ((("strategy", "S3", "multiplier"), 1), (("strategy", "S6", "min_positive_years"), 5),
                      (("conditional", "C5", "min_events"), 29), (("strategy", "S10", "max_drawdown_frac"), 0.25)):
        doc = pp.build_protocol()
        node = doc
        for k in path[:-1]:
            node = node[k]
        node[path[-1]] = val
        assert pp.protocol_id(doc) != base, path


def test_pass2_protocol_id_is_pinned_so_no_threshold_can_change_after_the_freeze():
    assert pp.PASS2_PROTOCOL_ID == PINNED_PROTOCOL_ID


IDS = {k: k[:3] * 8 for k in pc.PASS1_ID_KEYS}


def test_candidate_id_is_result_independent_kind_separated_and_protocol_bound():
    a = pc.candidate_id(pp.KIND_STRATEGY, "e1", "t1", IDS)
    assert a == pc.candidate_id(pp.KIND_STRATEGY, "e1", "t1", {**IDS, "extra_result": 123.0})
    assert a != pc.candidate_id(pp.KIND_CONDITIONAL, "e1", "t1", IDS)
    assert a != pc.candidate_id(pp.KIND_STRATEGY, "e2", "t1", IDS)
    assert a != pc.candidate_id(pp.KIND_STRATEGY, "e1", "t1", {**IDS, "universe_id": "z" * 24})
    assert a != pc.candidate_id(pp.KIND_STRATEGY, "e1", "t1", IDS, protocol_id="0" * 64)


def _edges(n_mod=3, n_weak=4, n_strong=0, kind="STRATEGY_EDGE"):
    out = []
    for i, cls in enumerate(["DISCOVERED_MODERATE"] * n_mod + ["DISCOVERED_WEAK"] * n_weak + ["DISCOVERED_STRONG"] * n_strong):
        tid = f"t{i}"
        out.append({"kind": kind, "edge_class": cls, "trial_id": tid, "config_id": f"cfg{i}", "scope": "AAA", "params": {"x": i},
                    "family": "S01", "edge_id": er.edge_id(kind, tid), **IDS})
    return out


def test_strategy_cohort_is_exactly_the_moderate_class_and_refuses_any_other_count(monkeypatch):
    edges = _edges()
    cells = [(i, {"config_id": f"cfg{i}", "params": {"x": i}, "family": "S01"}, "AAA", f"t{i}") for i in range(7)]
    ledger = [{"t": f"t{i}", "class": c} for i, c in enumerate(["DISCOVERED_MODERATE"] * 3 + ["DISCOVERED_WEAK"] * 4)]
    monkeypatch.setattr(pp, "EXPECTED_STRATEGY_COHORT", 3)
    got = pc.strategy_cohort(edges, cells, ledger)
    assert [c["trial_id"] for c in got] == ["t0", "t1", "t2"], "WEAK edges never advance"
    monkeypatch.setattr(pp, "EXPECTED_STRATEGY_COHORT", 4)
    with pytest.raises(pc.CohortRefusal, match="REFUSE before attempt #1"):
        pc.strategy_cohort(edges, cells, ledger)
    monkeypatch.setattr(pp, "EXPECTED_STRATEGY_COHORT", 3)
    with pytest.raises(pc.CohortRefusal, match="STRONG"):
        pc.strategy_cohort(_edges(n_strong=1), cells, ledger)
    with pytest.raises(pc.CohortRefusal, match="search-ledger"):
        pc.strategy_cohort(edges, cells, ledger[1:] + [{"t": "t9", "class": "DISCOVERED_MODERATE"}])


def test_conditional_cohort_is_exactly_the_strong_class_and_refuses_any_other_count(monkeypatch):
    edges = []
    for i, cls in enumerate(["DISCOVERED_STRONG"] * 2 + ["DISCOVERED_MODERATE", "DISCOVERED_WEAK"]):
        fid = f"f{i}"
        edges.append({"kind": "CONDITIONAL_EDGE", "edge_class": cls, "factor_id": fid, "condition_id": "c0", "family": "S05",
                      "params": {}, "horizon": 1, "edge_id": er.edge_id("CONDITIONAL_EDGE", fid), **IDS})
    ledger = [{"factor_id": e["factor_id"], "class": e["edge_class"]} for e in edges]
    monkeypatch.setattr(pp, "EXPECTED_CONDITIONAL_COHORT", 2)
    got = pc.conditional_cohort(edges, ledger, [{"condition_id": "c0"}], {"f0", "f1", "f2", "f3"})
    assert [c["factor_id"] for c in got] == ["f0", "f1"], "WEAK/MODERATE conditionals never advance"
    monkeypatch.setattr(pp, "EXPECTED_CONDITIONAL_COHORT", 3)
    with pytest.raises(pc.CohortRefusal, match="REFUSE before attempt #1"):
        pc.conditional_cohort(edges, ledger, [{"condition_id": "c0"}], {"f0", "f1", "f2", "f3"})


# ------------------------------------------------------------------------------------------ discovery fence

def _fence_frame(end_ts):
    return {"X": pd.DataFrame({"end_ts": pd.to_datetime(end_ts, utc=True)})}


@pytest.mark.parametrize("ts,label", [("2024-01-02", "CONTAMINATED_BY_REJECTED_RUN"),
                                      ("2025-06-02", "REMAINING_CONFIRMATION_RESERVE"), ("2026-04-01", "FINAL_HOLDOUT")])
def test_p01_p03_fence_refuses_contaminated_confirmation_and_holdout_rows(ts, label):
    with pytest.raises(pt.PartitionBreach, match=label):
        p1.fence_bars(_fence_frame(["2023-12-29", ts]))
    proof = p1.fence_bars(_fence_frame(["2023-12-28", "2023-12-29"]))
    assert proof["rows_in_forbidden_partitions"] == 0 and proof["rows_loaded"] == 2


def test_p01_p03_the_universe_loader_applies_the_fence_before_building_anything(monkeypatch, bars):
    leaky = dict(bars)
    leaky["X00"] = pd.concat([bars["X00"], bars["X00"].tail(1).assign(
        end_ts=pd.Timestamp("2024-01-03 05:00", tz="UTC"))], ignore_index=True)
    monkeypatch.setattr(ce, "load_bars", lambda *a, **k: leaky)
    with pytest.raises(pt.PartitionBreach):
        p1.load_discovery_universe({}, {})
    monkeypatch.setattr(ce, "load_bars", lambda *a, **k: bars)
    U, proof = p1.load_discovery_universe({}, {})
    assert proof["rows_in_forbidden_partitions"] == 0 and len(U.symbols) == len(bars)


def test_pass2_modules_never_import_the_acquisition_path():
    for f in EXP2.glob("p2_*.py"):
        text = f.read_text(encoding="utf-8")
        assert "acquire_symbol" not in text and "acquire_universe" not in text and "alpaca_historical" not in text, f.name


# --------------------------------------------------------------------------- S1 replay parity / S3-S5 stress / delay

def test_s1_replay_reproduces_accepted_metrics_and_any_difference_is_a_contradiction(U, meta):
    fam, par, sym = "S02", {"sma": 20}, "X01"
    m = _accepted(U, meta, fam, par, sym)
    rec = p2s.evaluate_strategy(_ctx(U, meta), _cand(fam, par, sym), m)
    assert rec["scenarios"]["S1"]["status"] == "PASS"
    for key, val in (("net_pnl_usd", m["net_pnl_usd"] + 1e-9), ("trade_count", m["trade_count"] + 1)):
        bad = {**m, key: val}
        with pytest.raises(p2s.BaselineContradiction):
            p2s.evaluate_strategy(_ctx(U, meta), _cand(fam, par, sym), bad)


def test_stress_at_multiplier_one_equals_the_accepted_baseline_and_x2_x3_cost_strictly_more(U, meta):
    fam, par, sym = "S01", {"lookback": 21, "cadence": "daily"}, "X02"
    sd, sig = U.sd[sym], U.build(sym, fam, par)
    m = _accepted(U, meta, fam, par, sym)
    net1, bench1, alpha1 = p2s.stressed_net_alpha(sd, sig.d, sig.s, 1)
    assert (net1, bench1) == (m["net_pnl_usd"], m["benchmark_net_pnl_usd"]), "x1 must reproduce Pass-1 economics"
    net2, bench2, _ = p2s.stressed_net_alpha(sd, sig.d, sig.s, pp.STRESS_2X)
    net3, bench3, _ = p2s.stressed_net_alpha(sd, sig.d, sig.s, pp.STRESS_3X)
    w0 = slice(sig.s + 1, None)
    notional = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, sig.s).notional_usd
    per_step = notional * (sm.COMMISSION_BPS + sm.SLIPPAGE_BPS) / 10_000.0   # one more multiple of commission + slippage
    assert net1 - net2 == pytest.approx(per_step, rel=0.02) and net2 - net3 == pytest.approx(per_step, rel=0.02)
    assert bench1 - bench2 > 0 and bench2 - bench3 > 0, "the matched benchmark pays the same stressed costs"
    w = slice(sig.s + 1, None)
    ref = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, sig.s, commission_bps=20.0, slippage_bps=10)
    assert net2 == float(ref.net[w].sum())


def test_p07_net_positive_but_benchmark_negative_stress_does_not_pass(U, meta):
    fam, par, sym = "S01", {"lookback": 21, "cadence": "daily"}, "UP"
    rec = p2s.evaluate_strategy(_ctx(U, meta), _cand(fam, par, sym), _accepted(U, meta, fam, par, sym))
    s3 = rec["scenarios"]["S3"]
    assert s3["net_pnl_usd"] > 0.0 and s3["alpha_usd"] < 0.0, "fixture must be net-positive and benchmark-negative"
    assert s3["status"] == "FAIL" and "S3" in rec["failed_gates"] and rec["verdict"] == pp.VERDICTS_STRATEGY[1]
    assert p2s.stress_scenario(5.0, 6.0, -1.0)["status"] == "FAIL"
    assert p2s.stress_scenario(-1.0, -9.0, 8.0)["status"] == "FAIL"
    assert p2s.stress_scenario(5.0, 1.0, 4.0)["status"] == "PASS"


def test_p08_p18_delay_is_one_whole_session_and_the_fill_is_still_strictly_after_the_decision(U):
    d = np.zeros(40, bool)
    d[10:13] = True
    dd = p2s.delayed_decisions(d)
    assert np.array_equal(dd[1:], d[:-1]) and not dd[0] and dd.sum() == d.sum()
    n = 40
    hm = lm = cm = np.full(n, 10_000_000, np.int64)
    base, delayed = sm.simulate(hm, lm, cm, d, 0), sm.simulate(hm, lm, cm, dd, 0)
    assert list(base.entries) == [11], "decision on bar 10 fills on bar 11, never bar 10"
    assert list(delayed.entries) == [12], "delayed decision on bar 11 fills on bar 12, never bar 11"
    assert list(delayed.entries) != list(base.entries)


def test_p17_strategy_pnl_comes_from_the_simulator_never_from_forward_return_labels(U, meta, monkeypatch):
    fam, par, sym = "S02", {"sma": 20}, "X03"
    m = _accepted(U, meta, fam, par, sym)

    def boom(*a, **k):
        raise AssertionError("Strategy path touched the diagnostic factor frame")
    monkeypatch.setattr(cd, "build_frame", boom)
    rec = p2s.evaluate_strategy(_ctx(U, meta), _cand(fam, par, sym), m)
    sd, sig = U.sd[sym], U.build(sym, fam, par)
    assert rec["scenarios"]["S3"]["net_pnl_usd"] == p2s.stressed_net_alpha(sd, sig.d, sig.s, 2)[0]
    assert p2s.stressed_net_alpha(sd, sig.d, sig.s, 1)[0] == m["net_pnl_usd"]
    fwd = float(np.sum(sd.c[2:] / sd.c[1:-1] - 1.0)) * 1e4
    assert abs(rec["scenarios"]["S3"]["net_pnl_usd"] - fwd) > 1.0


# ------------------------------------------------------------------------------------------ S2 / S6 / S7 / S8

@pytest.mark.parametrize("n,status", [(29, "FAIL"), (30, "PASS"), (500, "PASS")])
def test_s2_trade_floor_is_30(n, status):
    assert p2s.trade_scenario(n)["status"] == status


def _years(*vals):
    return {str(y): v for y, v in zip(pp.YEARS, vals)}


@pytest.mark.parametrize("vals,status,positive", [
    ((1, 1, 1, 1, 1, 1, -1, -1), "PASS", 6), ((1, 1, 1, 1, 1, -1, -1, -1), "FAIL", 5),
    ((1, 1, 1, 1, 1, 1, 1, 1), "PASS", 8), ((0, 0, 0, 0, 0, 0, 5, 5), "FAIL", 2)])
def test_p10_s6_needs_six_of_eight_positive_calendar_years(vals, status, positive):
    s6, _ = p2s.year_scenarios(_years(*vals))
    assert (s6["status"], s6["positive_years"]) == (status, positive)


def test_s6_years_absent_from_the_window_are_not_positive_and_never_dropped():
    s6, _ = p2s.year_scenarios({str(y): 3.0 for y in range(2018, 2024)} | {"2016": -1.0})
    assert s6["positive_years"] == 6 and s6["status"] == "PASS" and set(s6["year_alpha_usd"]) == {str(y) for y in pp.YEARS}
    assert s6["year_alpha_usd"]["2017"] == 0.0
    s6b, _ = p2s.year_scenarios({str(y): 3.0 for y in range(2019, 2024)})
    assert s6b["positive_years"] == 5 and s6b["status"] == "FAIL"


@pytest.mark.parametrize("vals,status,best", [
    ((10, -1, -1, -1, -1, -1, -1, -1), "FAIL", 2016), ((10, 5, 5, -1, -1, -1, -1, -1), "PASS", 2016),
    ((5, 5, 1, 1, 1, 1, 1, 1), "PASS", 2016), ((-1, -1, -1, -1, -1, -1, -1, 4), "FAIL", 2023)])
def test_p09_s7_removes_the_best_year_never_the_worst(vals, status, best):
    _, s7 = p2s.year_scenarios(_years(*vals))
    assert s7["status"] == status and s7["best_year"] == best
    assert s7["remaining_alpha_usd"] == sum(vals) - max(v for v in vals if v > 0)


def test_s7_remaining_alpha_excludes_exactly_the_best_year_and_ties_break_chronologically():
    _, s7 = p2s.year_scenarios(_years(3, 9, 9, 1, 1, 1, 1, 1))
    assert s7["best_year"] == 2017 and s7["remaining_alpha_usd"] == 3 + 9 + 5
    _, none = p2s.year_scenarios(_years(*(-1,) * 8))
    assert none["status"] == "BLOCKED"


@pytest.mark.parametrize("share,status", [(0.80, "PASS"), (0.8000001, "FAIL"), (0.95, "FAIL"), (0.5, "PASS"),
                                          (None, "BLOCKED")])
def test_p11_s8_regime_concentration_limit_is_080(share, status):
    assert p2s.regime_scenario(share, {})["status"] == status


# --------------------------------------------------------------------------------- S9 neighbourhood / S10 / S11

def test_p12_moderate_support_set_excludes_weak_cells(U, meta, monkeypatch):
    cfgs = ss.build_configs()[:3]
    cells = [(i, c, "AAA", f"t{i}") for i, c in enumerate(cfgs)]
    ledger = [{"c": 0, "s": "AAA", "d": "EVALUABLE", "class": "DISCOVERED_MODERATE"},
              {"c": 1, "s": "AAA", "d": "EVALUABLE", "class": "DISCOVERED_WEAK"},
              {"c": 2, "s": "AAA", "d": "EVALUABLE", "class": None},
              {"c": 2, "s": "BBB", "d": "NON_EVALUABLE", "class": None}]
    ctx = p2s.build_context(U, meta, cells, ledger)
    assert ctx.moderate == {(0, "AAA")} and ctx.moderate_count == {0: 1}
    assert ctx.evaluable == {0: 1, 1: 1, 2: 1}, "non-evaluable cells are not evaluable symbols"
    with pytest.raises(ValueError):
        p2s.build_context(U, meta, cells, [{"c": 0, "s": "AAA", "d": "EVALUABLE", "class": "DISCOVERED_STRONG"}])


def test_s9_support_share_threshold_and_not_applicable_without_neighbours():
    mod = {(1, "AAA"), (2, "AAA"), (7, "BBB")}
    assert p2s.neighbor_scenario([], "AAA", mod) == {"status": "NOT_APPLICABLE", "n_neighbors": 0}
    assert p2s.neighbor_scenario([1, 3, 4, 5], "AAA", mod)["status"] == "PASS"      # 1/4 = 0.25
    assert p2s.neighbor_scenario([1, 3, 4, 5, 6], "AAA", mod)["status"] == "FAIL"   # 1/5 < 0.25
    assert p2s.neighbor_scenario([7], "AAA", mod)["status"] == "FAIL", "support is same-symbol only"
    assert p2s.neighbor_scenario([1, 2], "AAA", mod)["share"] == 1.0


def test_p13_every_s9_neighbour_is_an_in_grid_adjacent_config():
    cfgs = ss.build_configs()
    nb = er.neighbor_map(cfgs)
    axis = {}
    for c in cfgs:
        for k, v in c["params"].items():
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                axis.setdefault((c["family"], k), set()).add(v)
    seen = 0
    for i, c in enumerate(cfgs):
        for j in nb[i]:
            o = cfgs[j]
            diff = [k for k in c["params"] if c["params"][k] != o["params"][k]]
            assert o["family"] == c["family"] and len(diff) == 1, (c, o)
            vals = sorted(axis[(c["family"], diff[0])])
            assert abs(vals.index(c["params"][diff[0]]) - vals.index(o["params"][diff[0]])) == 1, "adjacent grid values only"
            seen += 1
    assert seen > 500


def test_p13_the_pass2_strategy_context_uses_exactly_the_accepted_neighbour_authority(U, meta):
    cfgs = ss.build_configs()
    cells = [(i, c, "AAA", f"t{i}") for i, c in enumerate(cfgs)]
    ctx = p2s.build_context(U, meta, cells, [])
    assert ctx.neighbors == er.neighbor_map(cfgs)


def _spread(per_day, days, lead=10):
    net = np.zeros(300)
    net[lead:lead + days] = per_day
    return net


@pytest.mark.parametrize("per_day,days,status", [
    (-1000.0, 20, "PASS"),     # drawdown exactly 20,000 (= 20% of 100k), worst 5-session -5,000
    (-999.95, 20, "PASS"),     # drawdown 19,999
    (-1000.05, 20, "FAIL"),    # drawdown 20,001 > 20%
    (-1200.0, 5, "PASS"),      # worst rolling 5-session exactly -6,000 (= -6%)
    (-1201.0, 5, "FAIL"),      # worst rolling -6,005 < -6%
    (-20_000.0, 1, "FAIL")])   # one -20% day breaches the rolling bar even though drawdown == limit
def test_s10_main_risk_bar_is_20pct_of_100k_drawdown_and_minus_6pct_rolling_5_sessions(per_day, days, status):
    net = _spread(per_day, days)
    rec = p2s.risk_scenario(net)
    assert rec["status"] == status
    assert rec["max_drawdown_usd"] == pytest.approx(-per_day * days, abs=1e-6)
    assert rec["worst_rolling_net_usd"] == pytest.approx(per_day * min(days, 5), abs=1e-6)
    assert rec["initial_capital_usd"] == 100_000.0


def test_s10_uses_the_100k_capital_not_the_10k_allocation_and_needs_five_sessions():
    rec = p2s.risk_scenario(_spread(-1000.0, 20))
    assert rec["max_drawdown_frac_of_capital"] == 0.2 and rec["status"] == "PASS", "a 10k-budget reading would fail it"
    assert p2s.risk_scenario(np.array([1.0, 2.0, 3.0]))["status"] == "FAIL", "fewer than 5 sessions cannot prove the bar"


@pytest.mark.parametrize("n_ev,n_mod,cls,status", [
    (88, 0, "ZERO_REPLICATION_CONTRADICTION", "BLOCKED"), (88, 1, "SYMBOL_SPECIFIC", "CLASSIFIED"),
    (88, 2, "CLUSTER_REPLICATED", "CLASSIFIED"), (88, 43, "CLUSTER_REPLICATED", "CLASSIFIED"),
    (88, 44, "BROADLY_REPLICATED", "CLASSIFIED"), (10, 5, "BROADLY_REPLICATED", "CLASSIFIED")])
def test_s11_replication_classification(n_ev, n_mod, cls, status):
    r = p2s.replication_scenario(n_ev, n_mod)
    assert (r["class"], r["status"]) == (cls, status)


def _all_pass():
    return {g: {"status": "PASS"} for g in pp.STRATEGY_HARD_GATES} | {"S4": {"status": "FAIL"}, "S11": {"status": "CLASSIFIED"}}


def test_strategy_verdict_records_the_complete_ordered_failure_profile():
    assert p2s.strategy_verdict(_all_pass())[2] == pp.VERDICTS_STRATEGY[0]
    sc = _all_pass()
    sc["S10"]["status"], sc["S2"]["status"], sc["S6"]["status"] = "FAIL", "FAIL", "FAIL"
    failed, blocked, verdict = p2s.strategy_verdict(sc)
    assert failed == ["S2", "S6", "S10"] and verdict == pp.VERDICTS_STRATEGY[1] and not blocked
    sc = _all_pass()
    sc["S9"] = {"status": "NOT_APPLICABLE"}
    assert p2s.strategy_verdict(sc)[2] == pp.VERDICTS_STRATEGY[0], "N/A is neither a pass nor a failure"
    sc["S8"] = {"status": "BLOCKED"}
    assert p2s.strategy_verdict(sc)[2] == pp.VERDICTS_STRATEGY[2]
    sc["S3"] = {"status": "FAIL"}
    assert p2s.strategy_verdict(sc)[2] == pp.VERDICTS_STRATEGY[1], "a FAIL is conclusive over missing proof"
    sc = _all_pass()
    sc["S11"] = {"status": "BLOCKED"}
    assert p2s.strategy_verdict(sc)[2] == pp.VERDICTS_STRATEGY[2] and p2s.strategy_verdict(sc)[1] == ["S11"]
    sc = _all_pass()
    sc["S4"], sc["S11"] = {"status": "FAIL"}, {"status": "CLASSIFIED"}
    assert p2s.strategy_verdict(sc)[2] == pp.VERDICTS_STRATEGY[0], "3X is diagnostic, never a gate"


def test_strategy_record_has_every_scenario_labels_and_the_real_engine_outputs(U, meta):
    fam, par, sym = "S02", {"sma": 50}, "X05"
    rec = p2s.evaluate_strategy(_ctx(U, meta), _cand(fam, par, sym), _accepted(U, meta, fam, par, sym))
    assert set(rec["scenarios"]) == {f"S{i}" for i in range(1, 12)}
    assert rec["VALIDATION_STATUS"] == "NOT_VALIDATED" and rec["PROMOTION_AUTHORITY"] == "NONE"
    assert rec["verdict"] in pp.VERDICTS_STRATEGY and set(rec["failed_gates"]) <= set(pp.STRATEGY_HARD_GATES)
    json.dumps(rec, allow_nan=False)
    pr.assert_labels(rec)


def test_p22_a_record_claiming_validation_or_promotion_is_refused(U, meta):
    rec = p2s.evaluate_strategy(_ctx(U, meta), _cand("S02", {"sma": 50}, "X05"), _accepted(U, meta, "S02", {"sma": 50}, "X05"))
    for k, v in (("VALIDATION_STATUS", "VALIDATED"), ("PROMOTION_AUTHORITY", "PAPER_CANDIDATE")):
        with pytest.raises(pr.Pass2Refusal):
            pr.assert_labels({**rec, k: v})
    crec = {"kind": pp.KIND_CONDITIONAL, "candidate_id": "x", **pp.LABELS, "executable_pnl": True}
    with pytest.raises(pr.Pass2Refusal):
        pr.assert_labels(crec)
    pr.assert_labels({**crec, "executable_pnl": False})


def _rr(trial, passes3, a2, ad, lby, yrs, dd):
    return {"verdict": pp.VERDICTS_STRATEGY[0], "trial_id": trial,
            "rank_fields": {"passes_3x": passes3, "alpha_2x": a2, "alpha_delayed": ad, "leave_best_year_out_alpha": lby,
                            "positive_years": yrs, "max_drawdown_usd": dd}}


def test_strategy_ranking_is_lexicographic_read_only_and_survivors_only():
    recs = [_rr("e", False, 99, 99, 99, 8, 1), _rr("d", True, 5, 1, 1, 6, 9), _rr("c", True, 5, 2, 1, 6, 9),
            _rr("b", True, 5, 2, 3, 6, 9), _rr("a", True, 5, 2, 3, 7, 9), _rr("0", True, 5, 2, 3, 7, 4),
            _rr("Z", True, 5, 2, 3, 7, 4), {**_rr("rej", True, 999, 1, 1, 8, 0), "verdict": pp.VERDICTS_STRATEGY[1]}]
    snapshot = copy.deepcopy(recs)
    order = [r["trial_id"] for r in p2s.rank_strategy_survivors(recs)]
    assert order == ["0", "Z", "a", "b", "c", "d", "e"]
    assert recs == snapshot, "ranking never alters verdicts or identity"
    assert "rej" not in order


# ---------------------------------------------------------------------------------------------- ConditionalEdge

COND = next(c for c in ss.build_conditions(ss.build_configs()) if c["family"] == "S05" and c["params"] == {
    "period": 5, "entry_below": 10, "trend": "none"})
H = 5


@pytest.fixture(scope="module")
def cctx():
    seed = json.loads((EXP1 / "ALPHA_CENSUS_SEED_UNIVERSE_V2.json").read_text(encoding="utf-8"))
    universe = ss.build_universe(seed, {s: {"disposition": cdata.ELIGIBLE} for s in seed["symbols"]})
    bm = {"manifest_sha256": "0" * 64, "request_contract": cdata.REQUEST_CONTRACT}
    return cd.population_context(universe, ss.build_protocol(), ss.build_partitions(), bm)


@pytest.fixture(scope="module")
def raw(U):
    return p2c.RawRows(U, COND, H)


@pytest.fixture(scope="module")
def frame_aux(U):
    return cd.build_frame(U, COND, H)


def _pandas_effect(raw, mask):
    df = pd.DataFrame({"s": raw.symbol, "r": raw.ret, "e": raw.ev})[mask]
    df = df.assign(b=df.groupby("s")["r"].transform("mean"))
    ev = df[df["e"]]
    return (float((ev["r"] - ev["b"]).mean()) if len(ev) else None), int(len(ev))


def test_slice_estimator_equals_the_accepted_full_frame_effect_exactly(raw, frame_aux):
    frame, aux = frame_aux
    ev = cd.event_diagnostics(frame, aux)
    eff, n = raw.effect(np.ones(raw.n, bool))
    assert n == ev["event_count"] and eff == ev["direction_adjusted_effect"]
    assert np.array_equal(raw.label, frame["label_fwd_ret"].to_numpy())
    assert np.array_equal(raw.symbol, frame["symbol"].to_numpy()) and np.array_equal(raw.ev, frame["factor_value"].to_numpy() == 1.0)


def test_slice_estimator_matches_an_independent_pandas_reference_on_year_and_leave_out_slices(raw):
    for y in pp.YEARS:
        got, n = raw.effect(raw.year == y)
        ref, rn = _pandas_effect(raw, raw.year == y)
        assert n == rn and (got is None) == (ref is None)
        if got is not None:
            assert got == pytest.approx(ref, abs=1e-12)
    got, n = raw.effect(raw.year != 2019)
    ref, rn = _pandas_effect(raw, raw.year != 2019)
    assert n == rn and got == pytest.approx(ref, abs=1e-12)
    sym = "X03"
    keep = raw.ev & (raw.symbol != sym)
    assert float(raw.label[keep].mean()) == pytest.approx(_pandas_effect(raw, raw.symbol != sym)[0], abs=1e-12), \
        "removing one symbol leaves every other symbol's baseline unchanged"


class _Stub:
    """ConditionalContext with C1 stubbed out: isolates the C2-C8 slice/classification logic."""

    @staticmethod
    def make(U, cctx, *, ledger=None, strong=None, neighbors=None):
        cond_index = {COND["condition_id"]: 0}
        return p2c.ConditionalContext(U, cctx, {COND["condition_id"]: COND}, cond_index, neighbors or [[]], strong or set(),
                                      ledger or {}, {"status": "complete"}, cd.PermutationCache())


def _cand_c():
    return {"candidate_id": "k" * 32, "edge_id": "e" * 32, "factor_id": "f" * 32, "condition_id": COND["condition_id"],
            "family": "S05", "params": COND["params"], "horizon": H}


def _ledger(ok=True, eff=0.001, p=0.01):
    return {(COND["condition_id"], h): {"status": "succeeded" if ok else "not_evaluable", "effect": eff, "p_value": p}
            for h in pp.HORIZONS if h != H}


def _run_c(monkeypatch, U, cctx, frame_aux, *, events=None, **kw):
    frame, aux = frame_aux
    ev = events or cd.event_diagnostics(frame, aux)
    monkeypatch.setattr(p2c, "c1_baseline", lambda *a, **k: {"status": "PASS", "q_value": 0.05})
    cc = _Stub.make(U, cctx, **kw)
    return p2c.evaluate_conditional(cc, _cand_c(), {}, {"events": ev}, {})


def test_conditional_slices_follow_the_predeclared_rules_and_keep_every_denominator(monkeypatch, U, cctx, frame_aux, raw):
    rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger())
    sc = rec["scenarios"]
    assert set(sc) == {f"C{i}" for i in range(1, 9)} and rec["executable_pnl"] is False
    assert rec["VALIDATION_STATUS"] == "NOT_VALIDATED" and rec["PROMOTION_AUTHORITY"] == "NONE"
    assert set(sc["C2"]["per_year"]) == {str(y) for y in pp.YEARS}
    for y in pp.YEARS:
        ref, n = _pandas_effect(raw, raw.year == y)
        got = sc["C2"]["per_year"][str(y)]
        assert got["events"] == n and (got["effect"] == pytest.approx(ref, abs=1e-12) if ref is not None else got["effect"] is None)
    assert sc["C2"]["positive_years"] == sum(1 for v in sc["C2"]["per_year"].values() if v["effect"] and v["effect"] > 0)
    assert sc["C5"]["per_symbol"].keys() == cd.event_diagnostics(*frame_aux)["per_symbol"].keys()
    assert sc["C5"]["min_effect"] == min(v["effect"] for v in sc["C5"]["per_symbol"].values())
    assert sc["C5"]["per_symbol"][sc["C5"]["min_effect_symbol"]]["effect"] == sc["C5"]["min_effect"]
    assert sc["C6"]["status"] == "NOT_APPLICABLE" and "C6" in rec["not_applicable"]
    json.dumps(rec, allow_nan=False)


def test_p15_c4_top_symbol_share_above_half_fails_and_a_single_symbol_fails(monkeypatch, U, cctx, frame_aux):
    ev = copy.deepcopy(cd.event_diagnostics(*frame_aux))
    for share, reps, status in ((0.50, 5, "PASS"), (0.5000001, 5, "FAIL"), (0.9, 5, "FAIL"), (0.3, 1, "FAIL")):
        ev2 = {**ev, "top_symbol_event_share": share, "symbols_represented": reps}
        rec = _run_c(monkeypatch, U, cctx, frame_aux, events=ev2, ledger=_ledger())
        assert rec["scenarios"]["C4"]["status"] == status, (share, reps)


def test_p16_c5_a_left_out_slice_below_30_events_fails_and_is_never_dropped(monkeypatch, U, cctx, frame_aux, raw):
    ev = cd.event_diagnostics(*frame_aux)
    rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger())
    c5 = rec["scenarios"]["C5"]
    assert len(c5["per_symbol"]) == ev["symbols_represented"]
    tot = ev["event_count"]
    for s, v in c5["per_symbol"].items():
        assert v["remaining_events"] == tot - ev["per_symbol"][s]["n"]
        assert v["status"] == ("PASS" if v["effect"] is not None and v["effect"] > 0 and v["remaining_events"] >= 30 else "FAIL")
    # a floor above every slice's remaining events: every slice FAILS and every represented symbol stays listed
    monkeypatch.setattr(pp, "C_MIN_EVENTS", tot)
    low = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger())["scenarios"]["C5"]
    assert len(low["per_symbol"]) == ev["symbols_represented"] and low["status"] == "FAIL"
    assert all(v["status"] == "FAIL" for v in low["per_symbol"].values()) and low["failed_symbols"] == sorted(low["per_symbol"])


def test_c4_c5_a_single_represented_symbol_fails_both_cross_universe_gates(monkeypatch, bars, cctx):
    U1 = sg.Universe({"SPY": bars["SPY"]})
    fa = cd.build_frame(U1, COND, H)
    ev = cd.event_diagnostics(*fa)
    assert ev["symbols_represented"] == 1 and ev["event_count"] >= 30
    sc = _run_c(monkeypatch, U1, cctx, fa, ledger=_ledger())["scenarios"]
    assert sc["C4"]["status"] == "FAIL" and sc["C5"]["status"] == "FAIL"
    assert sc["C5"]["per_symbol"]["SPY"]["remaining_events"] == 0 and sc["C5"]["per_symbol"]["SPY"]["effect"] is None


def test_c3_leave_best_year_out_uses_the_best_contribution_year_and_requires_30_events(monkeypatch, U, cctx, frame_aux, raw):
    rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger())
    c3 = rec["scenarios"]["C3"]
    contrib = {y: float(raw.label[raw.ev & (raw.year == y)].sum()) for y in pp.YEARS}
    best = max((y for y in pp.YEARS if contrib[y] > 0), key=lambda y: (contrib[y], -y))
    assert c3["best_year"] == best
    ref, n = _pandas_effect(raw, raw.year != best)
    assert c3["remaining_events"] == n and c3["remaining_effect"] == pytest.approx(ref, abs=1e-12)
    assert c3["status"] == ("PASS" if ref > 0 and n >= 30 else "FAIL")


def test_c6_neighbour_support_needs_25pct_strong_neighbours_at_the_same_horizon(monkeypatch, U, cctx, frame_aux):
    for strong, status in (({(1, H)}, "PASS"), ({(1, 1)}, "FAIL"), (set(), "FAIL"), ({(1, H), (2, H)}, "PASS")):
        rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger(), strong=strong, neighbors=[[1, 2, 3, 4], [0], [0], [0], [0]])
        assert rec["scenarios"]["C6"]["status"] == status, strong
        assert ("C6" in rec["failed_gates"]) == (status == "FAIL")


def test_p14_conditional_neighbourhood_forbids_execution_only_parameters():
    conds = [dict(c) for c in ss.build_conditions(ss.build_configs())[:4]]
    bad = [{**conds[0], "params": {**conds[0]["params"], "exit_above": 70}}] + conds[1:]
    with pytest.raises(er.RegistryRefusal, match="execution-only"):
        er.conditional_neighbor_map(bad)
    with pytest.raises(er.RegistryRefusal, match="execution-only"):
        p2c.build_context(None, None, bad, [], {})
    ctx_nb = er.conditional_neighbor_map(ss.build_conditions(ss.build_configs()))
    assert len(ctx_nb) == 219 and any(ctx_nb)
    good = p2c.build_context(None, None, ss.build_conditions(ss.build_configs()), [], {})
    assert good.neighbors == ctx_nb


@pytest.mark.parametrize("ledger,cls", [
    (_ledger(True, 0.001, 0.05), "HORIZON_SUPPORTED"), (_ledger(True, 0.001, 0.2), "HORIZON_ISOLATED"),
    (_ledger(True, -0.001, 0.01), "HORIZON_ISOLATED"), (_ledger(False), "NOT_APPLICABLE")])
def test_c8_horizon_neighbourhood_is_classification_only(monkeypatch, U, cctx, frame_aux, ledger, cls):
    rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=ledger)
    assert rec["horizon_class"] == cls and rec["scenarios"]["C8"]["status"] == "CLASSIFIED"
    assert "C8" not in rec["failed_gates"] and "C8" not in pp.CONDITIONAL_HARD_GATES
    assert set(rec["scenarios"]["C8"]["adjacent_horizons"]) == {"3", "10"}


def test_c7_regime_concentration_is_a_warning_never_a_gate(monkeypatch, U, cctx, frame_aux):
    ev = copy.deepcopy(cd.event_diagnostics(*frame_aux))
    ev["per_regime"] = {"risk_on": {"n": 100, "effect": 0.01}, "risk_off": {"n": 10, "effect": 0.001}}
    rec = _run_c(monkeypatch, U, cctx, frame_aux, events=ev, ledger=_ledger())
    assert rec["regime_class"] == "REGIME_CONCENTRATED" and "C7" not in rec["failed_gates"]
    ev["per_regime"] = {"risk_on": {"n": 100, "effect": 0.01}, "risk_off": {"n": 100, "effect": 0.009}}
    assert _run_c(monkeypatch, U, cctx, frame_aux, events=ev, ledger=_ledger())["regime_class"] == "GENERAL_REGIME"


def test_conditional_verdict_failure_profile_and_ranking(monkeypatch, U, cctx, frame_aux):
    rec = _run_c(monkeypatch, U, cctx, frame_aux, ledger=_ledger())
    failed = [g for g in pp.CONDITIONAL_HARD_GATES if rec["scenarios"][g]["status"] == "FAIL"]
    assert rec["failed_gates"] == failed
    assert rec["verdict"] == (pp.VERDICTS_CONDITIONAL[1] if failed else pp.VERDICTS_CONDITIONAL[0])

    def r(fid, m, l, y, q):
        return {"verdict": pp.VERDICTS_CONDITIONAL[0], "factor_id": fid,
                "rank_fields": {"min_loo_effect": m, "leave_best_year_out_effect": l, "positive_years": y, "q_value": q}}
    recs = [r("e", 0.1, 0.1, 8, 0.01), r("d", 0.2, 0.1, 8, 0.01), r("c", 0.2, 0.3, 6, 0.01), r("b", 0.2, 0.3, 8, 0.05),
            r("a", 0.2, 0.3, 8, 0.01), {**r("x", 9, 9, 9, 0.0), "verdict": pp.VERDICTS_CONDITIONAL[1]}]
    snap = copy.deepcopy(recs)
    assert [x["factor_id"] for x in p2c.rank_conditional_survivors(recs)] == ["a", "b", "c", "d", "e"]
    assert recs == snap


# ----------------------------------------------------------------- C1 baseline parity on a real registered factor

@pytest.fixture(scope="module")
def c1_env(tmp_path_factory, U, cctx):
    tmp = tmp_path_factory.mktemp("c1")
    db, out, rec_dir = tmp / "f.sqlite", tmp / "art", tmp / "rec"
    spec = cd.factor_spec(COND, H, cctx)
    fid = cd.register_factor(db, spec)
    rec, _ = cd.resolve_factor(db, out, rec_dir, U, COND, H, cctx, origin="t", cache=cd.PermutationCache())
    store = ResearchResultStore(db)
    att = store.list_factor_evaluation_attempts(fid)[-1]
    q = 0.05
    fdr = {"status": "complete", "q_values": {fid: q}, "declared_factor_ids": [fid]}
    edge = {"events": rec["events"], "pvalue": rec["pvalue"], "q_value": q}
    return {"fid": fid, "rec": rec, "att": att, "fdr": fdr, "edge": edge}


_SHARED_CACHE = cd.PermutationCache()


def _c1(U, cctx, env, *, rec=None, edge=None, fdr=None, att=None, raw=None, frame_aux=None):
    cc = p2c.ConditionalContext(U, cctx, {COND["condition_id"]: COND}, {COND["condition_id"]: 0}, [[]], set(), {},
                                fdr or env["fdr"], _SHARED_CACHE)
    frame, aux = frame_aux or cd.build_frame(U, COND, H)
    cand = {"factor_id": env["fid"]}
    return p2c.c1_baseline(cc, cand, att or env["att"], rec or env["rec"], edge or env["edge"], frame, aux,
                           raw or p2c.RawRows(U, COND, H), COND, H)


def test_c1_reproduces_the_accepted_factor_evidence(U, cctx, c1_env):
    if c1_env["rec"]["pvalue"]["p_value"] > pp.C_HORIZON_P_MAX:
        pytest.skip("synthetic fixture factor is not significant at p<=0.10")
    out = _c1(U, cctx, c1_env)
    assert out["status"] == "PASS" and out["factor_id"] == c1_env["fid"]
    assert out["event_count"] == c1_env["rec"]["events"]["event_count"]


def test_c1_any_drift_in_identity_evidence_or_fdr_provenance_is_a_contradiction(U, cctx, c1_env):
    if c1_env["rec"]["pvalue"]["p_value"] > pp.C_HORIZON_P_MAX:
        pytest.skip("synthetic fixture factor is not significant at p<=0.10")
    rec, edge, fdr, att = c1_env["rec"], c1_env["edge"], c1_env["fdr"], c1_env["att"]
    fid = c1_env["fid"]
    cases = {
        "event_count": dict(rec={**rec, "events": {**rec["events"], "event_count": rec["events"]["event_count"] + 1}}),
        "effect": dict(rec={**rec, "events": {**rec["events"], "direction_adjusted_effect": rec["events"]["direction_adjusted_effect"] + 1e-9}}),
        "pvalue": dict(rec={**rec, "pvalue": {**rec["pvalue"], "p_value": 0.5}}),
        "attempt": dict(rec={**rec, "attempt_id": "other"}),
        "evaluation": dict(att={**att, "evaluation_id": "zz"}),
        "edge_q": dict(edge={**edge, "q_value": 0.06}),
        "fdr_incomplete": dict(fdr={**c1_env["fdr"], "status": "incomplete"}),
        "q_above_alpha": dict(fdr={**c1_env["fdr"], "q_values": {fid: 0.2}}, edge={**edge, "q_value": 0.2}),
        "not_declared": dict(fdr={**c1_env["fdr"], "declared_factor_ids": []}),
    }
    for name, kw in cases.items():
        with pytest.raises(p2c.FactorContradiction):
            _c1(U, cctx, c1_env, **kw)


# --------------------------------------------------------------------- durable attempts / resume / gates

def _fake_manifest(n_s=4, n_c=2):
    strat = [{"kind": pp.KIND_STRATEGY, "candidate_id": pc.candidate_id(pp.KIND_STRATEGY, f"e{i}", f"t{i}", IDS),
              "edge_id": f"e{i}", "trial_id": f"t{i}"} for i in range(n_s)]
    cond = [{"kind": pp.KIND_CONDITIONAL, "candidate_id": pc.candidate_id(pp.KIND_CONDITIONAL, f"ce{i}", f"f{i}", IDS),
             "edge_id": f"ce{i}", "factor_id": f"f{i}"} for i in range(n_c)]
    return pc.cohort_manifest(strat, cond)


BINDING = {"run_files_sha256": {"x": "0" * 64}}


def _store(tmp_path, manifest=None):
    manifest = manifest or _fake_manifest()
    st = ResearchResultStore(tmp_path / "p2.sqlite")
    pr.register(st, manifest, IDS, BINDING)
    return st, manifest


def _rec(c):
    return {"kind": c["kind"], "candidate_id": c["candidate_id"], "verdict": "X", **pp.LABELS,
            **({"executable_pnl": False} if c["kind"] == pp.KIND_CONDITIONAL else {})}


def _tasks(cands):
    return {c["candidate_id"]: {"cand": c} for c in cands}


def test_freeze_gate_requires_exact_registration_marker_and_zero_attempts(tmp_path):
    st, m = _store(tmp_path)
    assert pr.require_frozen(st, m, BINDING, allow_attempts=False)["attempts"] == 0
    assert all(t.startswith("p2-") for t in st.trial_attempt_digest(pp.EXPERIMENT_ID))
    assert st.trial_attempt_digest(ss.EXPERIMENT_ID) == {}, "no Pass-1 trial is ever created by Pass 2"
    with pytest.raises(pr.Pass2Refusal, match="marker"):
        pr.require_frozen(st, m, {"run_files_sha256": {"x": "1" * 64}}, allow_attempts=False)
    m2 = _fake_manifest(n_s=5)
    with pytest.raises(pr.Pass2Refusal, match="registered != cohort"):
        pr.require_frozen(st, m2, BINDING, allow_attempts=False)
    st.begin_attempts_bulk([pc.trial_id_of(m["strategy"][0]["candidate_id"])], origin="x")
    with pytest.raises(pr.Pass2Refusal, match="attempts already exist"):
        pr.require_frozen(st, m, BINDING, allow_attempts=False)
    pr.require_frozen(st, m, BINDING, allow_attempts=True)


def test_p19_a_robustness_result_never_becomes_a_strategy_trial_or_a_new_candidate(tmp_path):
    st, m = _store(tmp_path)
    cands = pr.all_candidates(m)
    pr.execute(st, cands, _tasks(cands), lambda t: _rec(t["cand"]), batch=3, log=lambda *_: None)
    digest = st.trial_attempt_digest(pp.EXPERIMENT_ID)
    assert set(digest) == {pc.trial_id_of(c["candidate_id"]) for c in cands} and len(digest) == 6, "no candidate minted by results"
    assert st.trial_attempt_digest(ss.EXPERIMENT_ID) == {}
    assert {t["experiment_id"] for t in st.list_trials()} == {pp.EXPERIMENT_ID}


def test_p21_terminal_candidates_are_never_reexecuted_and_resume_is_idempotent(tmp_path):
    st, m = _store(tmp_path)
    cands = pr.all_candidates(m)
    calls = []

    def fn(t):
        calls.append(t["cand"]["candidate_id"])
        return _rec(t["cand"])
    r1 = pr.execute(st, cands, _tasks(cands), fn, batch=2, log=lambda *_: None)
    assert r1["executed"] == 6 and len(calls) == 6
    r2 = pr.execute(st, cands, _tasks(cands), fn, batch=2, log=lambda *_: None)
    assert r2 == {"candidates": 6, "terminal_before": 6, "executed": 0} and len(calls) == 6
    assert all(d["attempts"] == 1 and d["succeeded"] == 1 for d in st.trial_attempt_digest(pp.EXPERIMENT_ID).values())


def test_partial_completion_restart_reuses_terminal_and_retries_only_the_interrupted(tmp_path):
    st, m = _store(tmp_path)
    cands = pr.all_candidates(m)
    calls = []

    def fn(t):
        calls.append(t["cand"]["candidate_id"])
        return _rec(t["cand"])
    pr.execute(st, cands[:3], _tasks(cands), fn, batch=2, log=lambda *_: None)        # first 3 terminal
    st.begin_attempts_bulk([pc.trial_id_of(cands[3]["candidate_id"])], origin="crash")  # process died mid-attempt
    first3 = list(calls)
    res = pr.execute(st, cands, _tasks(cands), fn, batch=2, log=lambda *_: None)
    assert res["terminal_before"] == 3 and res["executed"] == 3
    assert calls[:3] == first3 and calls[3:] == [c["candidate_id"] for c in cands[3:]], "terminal three never re-run"
    d3 = st.list_attempts(pc.trial_id_of(cands[3]["candidate_id"]))
    assert [a["status"] for a in d3] == ["failed", "succeeded"] and d3[0]["failure_reason"] == pp.INTERRUPTED_REASON
    assert [a["attempt_index"] for a in d3] == [1, 2], "same candidate, new attempt"
    assert all(d["succeeded"] == 1 for d in st.trial_attempt_digest(pp.EXPERIMENT_ID).values())


def test_a_defect_is_durably_failed_never_retried_by_outcome_and_later_candidates_stay_pending(tmp_path):
    st, m = _store(tmp_path)
    cands = pr.all_candidates(m)

    def fn(t):
        if t["cand"]["candidate_id"] == cands[2]["candidate_id"]:
            raise RuntimeError("boom")
        return _rec(t["cand"])
    with pytest.raises(pr.Pass2Defect, match="boom"):
        pr.execute(st, cands, _tasks(cands), fn, batch=6, log=lambda *_: None)
    att = st.list_attempts(pc.trial_id_of(cands[2]["candidate_id"]))
    assert [a["status"] for a in att] == ["failed"] and att[0]["failure_reason"].startswith("RuntimeError: boom")
    dig = st.trial_attempt_digest(pp.EXPERIMENT_ID)
    assert dig[pc.trial_id_of(cands[0]["candidate_id"])]["succeeded"] == 1 and not any(d["started"] for d in dig.values())
    with pytest.raises(pr.Pass2Refusal, match="never retried by outcome"):
        pr.execute(st, cands, _tasks(cands), lambda t: _rec(t["cand"]), batch=6, log=lambda *_: None)


def test_p20_the_complete_denominator_is_collected_including_rejected_candidates(tmp_path):
    st, m = _store(tmp_path)
    cands = pr.all_candidates(m)

    def fn(t):
        r = _rec(t["cand"])
        r["verdict"] = "PASS2_STRATEGY_REJECTED" if t["cand"]["candidate_id"] != cands[0]["candidate_id"] else "PASS2_STRATEGY_SURVIVOR"
        return r
    pr.execute(st, cands, _tasks(cands), fn, batch=6, log=lambda *_: None)
    got = pr.collect(st, cands)
    assert len(got) == len(cands) == 6 and sum(r["verdict"].endswith("REJECTED") for r in got) == 5
    (tmp_path / "second").mkdir()
    st2, m2 = _store(tmp_path / "second")
    pr.execute(st2, pr.all_candidates(m2)[:-1], _tasks(pr.all_candidates(m2)), fn, batch=6, log=lambda *_: None)
    with pytest.raises(pr.Pass2Refusal, match="not terminal"):
        pr.collect(st2, pr.all_candidates(m2))


def test_summary_denominators_preserve_every_candidate_and_count_failures_by_scenario():
    def s(v, failed, fam="S01", rep="CLUSTER_REPLICATED", sym="A", p3=False):
        return {"verdict": v, "failed_gates": failed, "blocked_gates": [], "not_applicable": [], "family": fam,
                "replication_class": rep, "symbol": sym, "pass_3x_cost_diagnostic": p3}

    def c(v, failed, fam="S05", h=1):
        return {"verdict": v, "failed_gates": failed, "blocked_gates": [], "not_applicable": [], "family": fam, "horizon": h,
                "regime_class": "GENERAL_REGIME", "horizon_class": "HORIZON_ISOLATED"}
    R, S = pp.VERDICTS_STRATEGY[1], pp.VERDICTS_STRATEGY[0]
    CR, CS = pp.VERDICTS_CONDITIONAL[1], pp.VERDICTS_CONDITIONAL[0]
    out = pr.summarize([s(R, ["S2", "S6"]), s(R, ["S2"]), s(S, [], p3=True)], [c(CR, ["C5"]), c(CS, [], h=5)])
    assert out["strategy"]["denominator"] == 3 and out["strategy"]["verdicts"] == {S: 1, R: 2}
    assert out["strategy"]["failures_by_scenario"]["S2"] == 2 and out["strategy"]["failures_by_scenario"]["S6"] == 1
    assert out["strategy"]["failure_profiles"] == {"NONE": 1, "S2": 1, "S2,S6": 1}
    assert out["conditional"]["denominator"] == 2 and out["conditional"]["failures_by_horizon"]["1"]["C5"] == 1
    assert out["strategy"]["survivors_by_3x_cost_diagnostic"] == {True: 1}
    assert out["VALIDATION_STATUS"] == "NOT_VALIDATED" and out["PROMOTION_AUTHORITY"] == "NONE"


def test_one_controller_owns_the_registry_at_a_time(tmp_path, monkeypatch):
    monkeypatch.setattr(pr, "RUN2", tmp_path)
    with pr.exclusive_owner("x"):
        with pytest.raises(pr.Pass2Refusal, match="another Pass-2 controller"):
            with pr.exclusive_owner("x"):
                pass
    with pr.exclusive_owner("x"):
        pass


def _git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)


def test_the_freeze_must_be_committed_and_unmodified_before_any_attempt(tmp_path):
    repo = tmp_path / "r"
    repo.mkdir()
    _git(repo, "init", "-q")
    _git(repo, "config", "user.email", "t@t")
    _git(repo, "config", "user.name", "t")
    f = repo / "PASS2_PREDECLARATION.json"
    f.write_text("{}\n", encoding="utf-8")
    with pytest.raises(pc.CohortRefusal, match="not committed"):
        pc.require_committed([f], repo)
    _git(repo, "add", f.name)
    with pytest.raises(pc.CohortRefusal, match="differs from HEAD"):
        pc.require_committed([f], repo)
    _git(repo, "commit", "-q", "-m", "x")
    assert len(pc.require_committed([f], repo)) == 40
    f.write_text('{"changed": 1}\n', encoding="utf-8")
    with pytest.raises(pc.CohortRefusal, match="differs from HEAD"):
        pc.require_committed([f], repo)


# ------------------------------------------------------------------------------------------ real evidence

@needs_run
def test_pass1_binding_is_internally_consistent_and_read_only():
    before = {n: p1.sha_raw(p1.RUN_DIR / n) for n in p1.RUN_FILES}
    b = p1.pass1_binding()
    assert b["attempt_counts"]["strategy_total"] == pp.EXPECTED_STRATEGY_ATTEMPTS
    assert b["attempt_counts"]["conditional_v3_total"] == pp.EXPECTED_CONDITIONAL_ATTEMPTS
    assert before == {n: p1.sha_raw(p1.RUN_DIR / n) for n in p1.RUN_FILES} == b["run_files_sha256"]
    tampered = copy.deepcopy(b)
    tampered["run_files_sha256"]["edge_registry_v3.jsonl"] = "0" * 64
    with pytest.raises(p1.Pass1EvidenceContradiction):
        p1.verify_binding(tampered)
    assert p1.verify_binding(b) == b


@needs_run
def test_read_only_store_cannot_write_to_a_pass1_registry():
    st = p1.ReadOnlyStore(p1.FACTOR_REGISTRY)
    assert st.list_factors(family=cd.FACTOR_FAMILY)
    with pytest.raises(Exception):
        st.register_hypothesis(hypothesis_id="x", experiment_id="y", hypothesis_text="z")


@needs_freeze
def test_real_cohorts_are_789_and_135_and_equal_the_committed_manifest():
    w = pr.load_world()
    m = pr.derive_manifest(w)
    assert (m["strategy_count"], m["conditional_count"]) == (789, 135)
    assert m == p1.load_json(pr.COHORT)
    predecl = p1.load_json(pr.PREDECL)
    assert predecl["PASS2_PROTOCOL_ID"] == pp.PASS2_PROTOCOL_ID and predecl["protocol"] == pp.build_protocol()
    assert predecl["cohort"]["cohort_manifest_sha256"] == pp.sha256_canonical(m)
    assert predecl["robustness_attempt_count"] == 0 and p1.load_json(pr.FREEZE_PROOF)["robustness_attempts_at_freeze"] == 0
    assert len({c["candidate_id"] for c in pr.all_candidates(m)}) == 924
    ids = {e["edge_id"] for e in w["edges"] if e["kind"] == "STRATEGY_EDGE" and e["edge_class"] == "DISCOVERED_WEAK"}
    assert not ids & {c["edge_id"] for c in m["strategy"]}, "WEAK StrategyEdges never advance"


@needs_freeze
def test_real_engine_reproduces_accepted_baselines_for_a_cohort_sample():
    w = pr.load_world()
    m = pr.derive_manifest(w)
    U, fence = p1.load_discovery_universe(w["uni"], w["bm"])
    assert fence["rows_in_forbidden_partitions"] == 0 and fence["max_end_ts"] < "2024-01-01"
    sc = p2s.build_context(U, ce.symbol_meta(w["bm"], w["prot"]), w["cells"], w["sledger"])
    edge = {e["trial_id"]: e for e in w["edges"] if e["kind"] == "STRATEGY_EDGE"}
    for c in m["strategy"][::40]:
        rec = p2s.evaluate_strategy(sc, c, edge[c["trial_id"]]["metrics"])
        assert rec["scenarios"]["S1"]["status"] == "PASS" and rec["candidate_id"] == c["candidate_id"]
        assert rec["verdict"] in pp.VERDICTS_STRATEGY
        assert rec["rank_fields"]["alpha_2x"] == rec["scenarios"]["S3"]["alpha_usd"]


# --------------------------------------------------------------------------- truth disposition (documentation)

DISP = EXP2 / "results" / "PASS2_PROCESS_DISPOSITION.json"
RESULT_DOC = TESTS.parents[1] / "docs" / "research" / "ALPHA_EDGE_PASS2_ROBUSTNESS_PURGE_01_RESULT.md"
needs_results = pytest.mark.skipif(not DISP.exists(), reason="results not present")


@needs_results
def test_disposition_declares_the_strategy_process_contamination_and_zero_advancement():
    d = json.loads(DISP.read_text(encoding="utf-8"))
    assert d["STRATEGY_PASS2_PROCESS_STATUS"] == "PROCESS_CONTAMINATED_PRE_FREEZE_REAL_COHORT_SMOKE"
    assert d["STRATEGY_ADVANCEMENT_AUTHORITY"] == "NONE" and d["STRATEGY_CANDIDATES_AUTHORIZED_TO_ADVANCE"] == 0
    assert d["STRATEGY_PASS2_SATISFIES_FREEZE_BEFORE_ANY_RESULT_CONTRACT"] is False
    assert d["strategy_result_rows"]["preserved_unaltered"] is True and d["strategy_result_rows"]["survivors"] == 0
    assert "false-negative" in d["risk_analysis"] and "cannot create a false-positive" in d["risk_analysis"]
    c = d["conditional"]
    assert d["CONDITIONAL_PASS2_STATUS"] == "LOCALLY_COMPLETE_PENDING_FINAL_CHATGPT_ACCEPTANCE"
    assert (c["denominator"], c["survivors"], c["rejected"], c["blocked"]) == (135, 18, 117, 0)
    assert c["VALIDATION_STATUS"] == "NOT_VALIDATED" and c["PROMOTION_AUTHORITY"] == "NONE" and c["EXECUTABLE_PNL"] is False
    assert d["attempt_counts"] == {"pass1_strategy": 38192, "pass1_conditional_v3": 1095, "pass2": 924}


@needs_results
def test_disposition_binds_the_unaltered_result_artifacts_and_the_exact_six_s2_near_misses():
    d = json.loads(DISP.read_text(encoding="utf-8"))
    for name, h in d["bound_artifacts_sha256_lf"].items():
        assert p1.sha_lf(EXP2 / "results" / name) == h, name
    rows = [json.loads(x) for x in (EXP2 / "results" / "strategy_robustness_ledger.jsonl").read_text(encoding="utf-8").splitlines()]
    only_s2 = sorted(r["candidate_id"] for r in rows if r["failed_gates"] == ["S2"])
    near = d["strategy_diagnostic_near_misses_s2_only"]
    assert len(rows) == 789 and len(only_s2) == 6 == len(near) and sorted(n["candidate_id"] for n in near) == only_s2
    assert all(n["role"].startswith("DIAGNOSTIC_NEAR_MISS_NOT_SURVIVOR") for n in near)
    assert not any(r["verdict"] == pp.VERDICTS_STRATEGY[0] for r in rows)


def test_result_doc_distinguishes_durable_freeze_from_the_pre_freeze_smoke_and_drops_the_misleading_claim():
    t = RESULT_DOC.read_text(encoding="utf-8")
    for must in ("PROCESS_CONTAMINATED_PRE_FREEZE_REAL_COHORT_SMOKE", "STRATEGY_ADVANCEMENT_AUTHORITY = NONE",
                 "STRATEGY_CANDIDATES_AUTHORIZED_TO_ADVANCE = 0", "LOCALLY_COMPLETE_PENDING_FINAL_CHATGPT_ACCEPTANCE",
                 "does **not** satisfy the freeze-before-any-result contract", "false-negative", "false-positive advancement",
                 "NOT survivors and NOT Confirmation candidates", "first **durable** Pass-2 attempt", "EXECUTABLE_PNL = false"):
        assert must in t, must
    assert "were committed before robustness attempt #1" not in t
