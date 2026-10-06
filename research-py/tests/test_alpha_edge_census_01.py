"""Alpha Edge Census Pass 1 invariants: authority, causality, execution, registry, resume, edge identity."""

from __future__ import annotations

import copy
import datetime as dt
import json
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

EXP = Path(__file__).resolve().parents[1] / "experiments" / "alpha_edge_census_01"
sys.path.insert(0, str(EXP))

import calendar_authority as cal  # noqa: E402
import census as ce  # noqa: E402
import conditional as cd  # noqa: E402
import data as cdata  # noqa: E402
import edge_registry as er  # noqa: E402
import partitions as pt  # noqa: E402
import search_space as ss  # noqa: E402
import signals as sg  # noqa: E402
import simulate as sm  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.execution_pricing import conservative_fill_price_micros  # noqa: E402

N_SYM = 21
SESSIONS = sg.SESSIONS


# ----------------------------------------------------------------------------------------------- fixtures

def synth_bars(symbol: str, seed: int, *, phi=-0.15, mu=0.0003, sig=0.012, start=0, price0=50.0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    n = len(SESSIONS) - start
    r = np.zeros(n)
    eps = rng.standard_normal(n)
    for i in range(1, n):
        r[i] = mu + phi * r[i - 1] + sig * eps[i]
    close = np.round(price0 * np.exp(np.cumsum(r)), 2)
    open_ = np.round(np.concatenate([[close[0]], close[:-1]]) * (1 + 0.002 * rng.standard_normal(n)), 2)
    hi = np.round(np.maximum(open_, close) * (1 + np.abs(rng.standard_normal(n)) * 0.004), 2)
    lo = np.round(np.minimum(open_, close) * (1 - np.abs(rng.standard_normal(n)) * 0.004), 2)
    hi, lo = np.maximum(hi, np.maximum(open_, close)), np.minimum(lo, np.minimum(open_, close))
    vol = np.round(1e6 * (1 + rng.random(n)), 0)
    ts = [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[start:]]
    return pd.DataFrame({"symbol": symbol, "end_ts": ts, "open": open_, "high": hi, "low": lo, "close": close, "volume": vol})


@pytest.fixture(scope="module")
def bars():
    out = {"SPY": synth_bars("SPY", 1, phi=0.0, mu=0.0004, sig=0.009, price0=200.0)}
    for i in range(N_SYM):
        late = (i == N_SYM - 1)
        out[f"X{i:02d}"] = synth_bars(f"X{i:02d}", 100 + i, phi=-0.2 if i % 2 else 0.1, start=700 if late else 0)
    return out


@pytest.fixture(scope="module")
def U(bars):
    return sg.Universe(bars)


def perturbed(bars, boundary_ord: int):
    out = {}
    for s, b in bars.items():
        rng = np.random.default_rng(7)
        o = np.array([sg.SESSION_INDEX[t.date()] for t in b["end_ts"]])
        f = np.where(o > boundary_ord, rng.uniform(0.3, 3.0, len(b)), 1.0)
        b2 = b.copy()
        for c in ("open", "high", "low", "close"):
            b2[c] = (b[c] * f).round(6)
        b2["volume"] = b["volume"] * np.where(o > boundary_ord, rng.uniform(0.2, 5.0, len(b)), 1.0)
        out[s] = b2
    return out


def first_configs(per_family=2):
    seen, out = {}, []
    for c in ss.build_configs():
        if seen.get(c["family"], 0) < per_family:
            seen[c["family"]] = seen.get(c["family"], 0) + 1
            out.append(c)
    return out


# --------------------------------------------------------------------------------------- authority / identity

def test_frozen_manifests_match_regenerated_authority():
    uni = json.loads((EXP / "ALPHA_CENSUS_UNIVERSE_V1.json").read_text())
    part = json.loads((EXP / "ALPHA_CENSUS_PARTITIONS_V1.json").read_text())
    prot = json.loads((EXP / "ALPHA_CENSUS_PROTOCOL_V1.json").read_text())
    space = json.loads((EXP / "ALPHA_CENSUS_SEARCH_SPACE_V1.json").read_text())
    assert part == ss.build_partitions() and prot == ss.build_protocol()
    assert ss.build_search_space(uni, part, prot) == space
    assert len(uni["symbols"]) == 88 and uni["symbols"] == sorted(set(uni["symbols"]))
    assert "AAPL" in uni["symbols"]
    assert uni["universe_source_kind"] == "current_enabled_equity_registry_snapshot_v1"
    assert uni["point_in_time_membership"] is False
    assert uni["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"


def test_search_space_shape_and_unique_identity():
    space = json.loads((EXP / "ALPHA_CENSUS_SEARCH_SPACE_V1.json").read_text())
    cfgs = space["configs"]
    assert len({c["config_id"] for c in cfgs}) == len(cfgs) == space["config_count"]
    assert {c["family"] for c in cfgs} == {f"S{i:02d}" for i in range(1, 21)}
    ids = {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}
    tids = [t for _c, _s, t in ss.iter_cells(cfgs, json.loads((EXP / "ALPHA_CENSUS_UNIVERSE_V1.json").read_text())["symbols"], ids)]
    assert len(tids) == len(set(tids)) == space["strategy_cell_count"]
    assert space["conditional_query_count"] == 6 * space["strategy_cell_count"]


def test_identity_is_param_order_invariant_and_param_sensitive():
    ids = {"universe_id": "u", "partitions_id": "p", "protocol_id": "q"}
    c = {"config_id": "x", "family": "S01", "scope": "symbol", "params": {"a": 1, "b": 2}}
    c_perm = {**c, "params": {"b": 2, "a": 1}}
    c_other = {**c, "params": {"a": 1, "b": 3}}
    t = lambda cfg: ss.trial_id(ss.trial_identity(cfg, "SPY", ids))  # noqa: E731
    assert t(c) == t(c_perm) and t(c) != t(c_other)
    assert ss.config_id("S01", {"a": 1, "b": 2}) == ss.config_id("S01", {"b": 2, "a": 1})
    assert ss.config_id("S01", {"a": 1, "b": 2}) != ss.config_id("S01", {"a": 1, "b": 3})
    assert ss.trial_id(ss.trial_identity(c, "SPY", ids)) != ss.trial_id(ss.trial_identity(c, "QQQ", ids))


def test_edge_id_is_result_independent():
    a = er.edge_id("STRATEGY_EDGE", "ace1-abc")
    assert a == er.edge_id("STRATEGY_EDGE", "ace1-abc")
    assert a != er.edge_id("CONDITIONAL_EDGE", "ace1-abc", 1)
    assert er.edge_id("CONDITIONAL_EDGE", "ace1-abc", 1) != er.edge_id("CONDITIONAL_EDGE", "ace1-abc", 2)
    import inspect
    assert list(inspect.signature(er.edge_id).parameters) == ["kind", "trial_id", "horizon"]


def test_protocol_declares_frozen_economics_and_non_executable_conditional():
    prot = ss.build_protocol()
    ec = prot["execution_contract"]
    assert ec["execution_pricing"]["slippage_bps"] == 5 and ec["execution_pricing"]["volatility_mult_bps"] == 0
    assert prot["flag_thresholds"]["tiny_sample_trades"] == 30
    assert prot["metric_definitions"]["dsr"].startswith("DEFERRED_FULL_POPULATION")
    assert prot["chunking"]["cells_per_chunk"] == 500
    assert "label only" in prot["conditional_edge"]["fwd_ret"] and prot["sizing"]["production_default"] is False


# ----------------------------------------------------------------------------------- partitions / data / SIP

def test_calendar_pin_and_session_grid_end_at_discovery_boundary():
    assert cal.CONTENT_SHA256 == cal.EXPECTED_CONTENT_SHA256
    assert SESSIONS[0] >= dt.date(2016, 1, 1) and SESSIONS[-1] == dt.date(2024, 12, 31)


@pytest.mark.parametrize("ts", ["2025-01-01", "2025-06-30", "2026-02-27", "2026-03-01", "2026-09-01"])
def test_partition_fence_refuses_reserve_and_holdout(ts):
    pt.require_discovery_only(pd.Series(["2024-12-31T05:00:00+00:00"]), what="ok")
    with pytest.raises(pt.PartitionBreach):
        pt.require_discovery_only(pd.Series(["2024-12-30T05:00:00+00:00", ts + "T05:00:00+00:00"]), what="bad")


def test_partition_fence_refuses_empty_input():
    with pytest.raises(pt.PartitionBreach):
        pt.require_discovery_only(pd.Series([], dtype=object), what="empty")


def test_symbol_data_refuses_non_session_and_post_discovery_dates(bars):
    b = bars["X00"].copy()
    b.loc[5, "end_ts"] = pd.Timestamp("2025-01-02", tz="UTC") + pd.Timedelta(hours=5)
    with pytest.raises(RuntimeError, match="canonical session"):
        sg.SymbolData("X00", b)
    b = bars["X00"].copy()
    b.loc[5, "end_ts"] = pd.Timestamp("2020-07-04", tz="UTC") + pd.Timedelta(hours=5)
    with pytest.raises(RuntimeError, match="canonical session"):
        sg.SymbolData("X00", b)


def test_sip_all_contract_refuses_other_feed_or_adjustment():
    good = {"source_attestation": {"feed": "sip", "adjustment_mode": "all", "source_provider_id": "alpaca",
                                   "requested_start_utc": "2016-01-01T00:00:00+00:00",
                                   "requested_end_utc": "2025-01-01T00:00:00+00:00"}}
    cdata.require_sip_all_contract(good)
    for k, v in (("feed", "iex"), ("adjustment_mode", "raw"), ("source_provider_id", "other"),
                 ("requested_end_utc", "2026-03-02T00:00:00+00:00")):
        bad = copy.deepcopy(good)
        bad["source_attestation"][k] = v
        with pytest.raises(SystemExit):
            cdata.require_sip_all_contract(bad)
    with pytest.raises(SystemExit):
        cdata.require_sip_all_contract({})
    assert cdata.REQUEST_CONTRACT["feed"] == "sip" and cdata.REQUEST_CONTRACT["adjustment"] == "all"


def test_acquisition_never_falls_back_to_iex_and_keeps_symbol(tmp_path, monkeypatch):
    import mqk_research.data.alpaca_historical as ah
    calls = []

    def boom(**kw):
        calls.append(kw["feed"])
        raise ah.AlpacaHistoricalExtractionError("sip denied")

    monkeypatch.setattr(ah, "extract_research_bars_with_provenance", boom)
    monkeypatch.setattr(cdata, "load_alpaca_env", lambda: None)
    monkeypatch.setattr(cdata.time, "sleep", lambda s: None)
    rec = cdata.acquire_symbol("SPY", tmp_path / "SPY")
    assert calls == ["sip"] and rec["disposition"] == "DATA_UNAVAILABLE_PROVIDER_ERROR"
    assert (tmp_path / "SPY" / "status.json").exists()

    def ca(**kw):
        calls.append(kw["feed"])
        raise ah.CorporateActionReviewRequired("needs review")

    monkeypatch.setattr(ah, "extract_research_bars_with_provenance", ca)
    rec = cdata.acquire_symbol("QQQ", tmp_path / "QQQ")
    assert rec["disposition"] == "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION" and set(calls) == {"sip"}


# ----------------------------------------------------------------------------------------- simulator

def ref_sim(hm, lm, cm, d):
    """Independent scalar reference: bar b fills the decision taken after bar b-1."""
    n = len(cm)
    in_run, qty = False, 0
    net, gross, cost, held = np.zeros(n), np.zeros(n), np.zeros(n), np.zeros(n, bool)
    run_pnl, entries, exits, cur = [], [], [], 0.0
    for b in range(n):
        held[b] = qty > 0
        g = qty * (int(cm[b]) - int(cm[b - 1])) / 1e6 if b > 0 else 0.0
        c = 0.0
        want = bool(d[b - 1]) if b > 0 else False
        if want and not in_run:
            in_run, qty = True, 10_000_000_000 // int(cm[b - 1])
            if qty > 0:
                buy = conservative_fill_price_micros(high_micros=int(hm[b]), low_micros=int(lm[b]), close_micros=int(cm[b]),
                                                     side="buy", slippage_bps=5, volatility_mult_bps=0)
                c = (qty * (buy - int(cm[b])) + qty * buy * 10 / 10_000) / 1e6
                entries.append(b)
                cur = 0.0
        elif not want and in_run:
            in_run = False
            if qty > 0:
                sell = conservative_fill_price_micros(high_micros=int(hm[b]), low_micros=int(lm[b]), close_micros=int(cm[b]),
                                                      side="sell", slippage_bps=5, volatility_mult_bps=0)
                c = (qty * (int(cm[b]) - sell) + qty * sell * 10 / 10_000) / 1e6
                exits.append(b)
            qty_after = 0
        gross[b], cost[b], net[b] = g, c, g - c
        if in_run and qty > 0 or (b in exits):
            cur += net[b]
        if b in exits:
            run_pnl.append(cur)
        if not in_run:
            qty = 0
    return net, gross, cost, held, entries, exits, run_pnl


def _micro(b):
    return tuple(np.rint(b[c].to_numpy() * 1e6).astype(np.int64) for c in ("high", "low", "close"))


def test_fill_vector_matches_rust_mirror_scalar_function():
    rng = np.random.default_rng(3)
    c = rng.integers(1_000_000, 900_000_000, 500).astype(np.int64)
    h = c + rng.integers(0, 40_000_000, 500)
    low = np.maximum(c - rng.integers(0, 40_000_000, 500), 1)
    buy, sell = sm.conservative_fills(h, low, c)
    for i in range(500):
        kw = dict(high_micros=int(h[i]), low_micros=int(low[i]), close_micros=int(c[i]), slippage_bps=5, volatility_mult_bps=0)
        assert buy[i] == conservative_fill_price_micros(side="buy", **kw)
        assert sell[i] == conservative_fill_price_micros(side="sell", **kw)


@pytest.mark.parametrize("seed", [0, 1, 2, 3])
def test_vector_simulator_matches_scalar_reference(seed):
    rng = np.random.default_rng(seed)
    b = synth_bars("T", seed, price0=(30000.0 if seed == 3 else 60.0))  # seed 3: qty == 0 (price > budget)
    hm, lm, cm = _micro(b)
    n = len(cm)
    d = np.zeros(n, bool)
    state = False
    for i in range(n):
        if rng.random() < 0.08:
            state = not state
        d[i] = state
    out = sm.simulate(hm, lm, cm, d, 0)
    net, gross, cost, held, entries, exits, runs = ref_sim(hm, lm, cm, d)
    np.testing.assert_allclose(out.net, net, rtol=0, atol=1e-6)
    np.testing.assert_allclose(out.gross, gross, atol=1e-6)
    np.testing.assert_allclose(out.cost, cost, atol=1e-6)
    assert list(out.entries) == entries and list(out.exits) == exits
    np.testing.assert_allclose(out.run_pnl, runs, atol=1e-6)
    assert (out.held_before == held).all()
    if seed == 3:
        assert len(entries) == 0 and np.all(out.net == 0)  # false-positive guard: zero-qty run skipped
    else:
        assert len(entries) > 10 and out.cost.sum() > 0


def test_fills_are_strictly_after_signal_bar_and_never_same_bar():
    b = synth_bars("T", 5)
    hm, lm, cm = _micro(b)
    n = len(cm)
    d = np.zeros(n, bool)
    d[100:140] = True
    out = sm.simulate(hm, lm, cm, d, 99)
    assert list(out.entries) == [101] and list(out.exits) == [141]
    assert out.net[100] == 0 and out.gross[101] == 0  # nothing happens on the signal bar; entry bar has no prior position
    qty = 10_000_000_000 // cm[100]  # qty sized on the completed signal bar
    buy = int(sm.conservative_fills(hm, lm, cm)[0][101])
    expected_adv = qty * (buy - int(cm[101])) / 1e6
    assert out.cost[101] == pytest.approx(expected_adv + qty * buy * 10 / 10_000 / 1e6)
    assert out.gross[102] == pytest.approx(qty * (int(cm[102]) - int(cm[101])) / 1e6)


def test_simulator_net_prefix_is_independent_of_future_bars():
    b = synth_bars("T", 6)
    hm, lm, cm = _micro(b)
    d = (np.arange(len(cm)) // 17) % 2 == 0
    T = 900
    a = sm.simulate(hm, lm, cm, d, 0)
    hm2, lm2, cm2 = hm.copy(), lm.copy(), cm.copy()
    hm2[T + 1:], lm2[T + 1:], cm2[T + 1:] = hm[T + 1:] * 3, lm[T + 1:] * 3, cm[T + 1:] * 3
    d2 = d.copy()
    d2[T:] = ~d2[T:]
    b2 = sm.simulate(hm2, lm2, cm2, d2, 0)
    np.testing.assert_array_equal(a.net[: T + 1], b2.net[: T + 1])


def test_capital_sizing_whole_shares_constant_budget_no_compounding():
    b = synth_bars("T", 8)
    hm, lm, cm = _micro(b)
    d = np.zeros(len(cm), bool)
    d[200:260] = True
    d[400:460] = True
    out = sm.simulate(hm, lm, cm, d, 199)
    # per-run notional is bounded by the fixed budget (floor shares) regardless of prior P&L
    for e, x in zip(out.entries, out.exits):
        q = 10_000_000_000 // cm[e - 1]
        assert q * cm[e - 1] <= 10_000_000_000 < (q + 1) * cm[e - 1]
        assert out.gross[e + 1 : x].sum() == pytest.approx(q * (int(cm[x - 1]) - int(cm[e])) / 1e6)
    assert sm.BUDGET_USD == 10_000.0 and sm.CAPITAL_USD == 100_000.0 and sm.BUDGET_MICROS == 10_000_000_000


def test_matched_benchmark_enters_after_first_defined_bar_and_pays_costs():
    b = synth_bars("T", 9)
    hm, lm, cm = _micro(b)
    n = len(cm)
    s = 250
    bd = sm.benchmark_d(n, s)
    assert not bd[:s].any() and bd[s:].all()
    bo = sm.simulate(hm, lm, cm, bd, s)
    assert list(bo.entries) == [s + 1] and len(bo.exits) == 0
    assert bo.cost[s + 1] > 0 and bo.cost.sum() == pytest.approx(bo.cost[s + 1])  # entry cost only, no liquidation cost
    # a strategy identical to the benchmark has zero net alpha
    years = np.array([t.year for t in b["end_ts"]])
    m = sm.metrics(bo, bo, years, np.full(n, 2, np.int8), capital_usd=1e5, budget_usd=1e4, window_start=s + 1)
    assert m["net_alpha_usd"] == 0.0


def test_metrics_values_against_hand_computation():
    n = 6
    z = np.zeros(n)
    st = sm.SimOut(net=np.array([0, 1.0, -0.5, 2.0, 0, 0]), gross=z, cost=np.array([0, 0.1, 0, 0.2, 0, 0]),
                   held_before=np.array([0, 0, 1, 1, 1, 0], bool), entries=np.array([1]), exits=np.array([4]),
                   run_pnl=np.array([2.5]), notional_usd=20_000.0, start=1)
    bn = sm.SimOut(net=np.array([0, 0.5, 0.5, 0.5, 0, 0]), gross=z, cost=np.array([0, 0.05, 0, 0, 0, 0]),
                   held_before=np.zeros(n, bool), entries=np.array([1]), exits=np.zeros(0, np.int64), run_pnl=z[:0],
                   notional_usd=0.0, start=1)
    yrs = np.array([2020, 2020, 2020, 2021, 2021, 2021])
    m = sm.metrics(st, bn, yrs, np.array([0, 1, 1, 1, 0, 2], np.int8), capital_usd=1000.0, budget_usd=100.0, window_start=1)
    assert m["net_pnl_usd"] == pytest.approx(2.5) and m["benchmark_net_pnl_usd"] == pytest.approx(1.5)
    assert m["net_alpha_usd"] == pytest.approx(1.0) and m["net_alpha_return"] == pytest.approx(0.001)
    assert m["max_drawdown_usd"] == pytest.approx(0.5) and m["turnover"] == pytest.approx(200.0)
    assert m["trade_count"] == 1 and m["round_trips"] == 1 and m["wins"] == 1 and m["losses"] == 0
    assert m["year_alpha_usd"]["2020"] == pytest.approx((1 - 0.5) + (-0.5 - 0.5))
    assert m["year_alpha_usd"]["2021"] == pytest.approx(2.0 - 0.5)
    assert m["exposure"] == pytest.approx(3 / 5)
    assert m["cost_fragile"] is False  # alpha 1.0 minus cost gap 0.25 stays positive


# ----------------------------------------------------------------- causal signals / fold isolation (synthetic)

def prefix_invariant(U1, U2, symbol, family, params, boundary_ord):
    a, b = U1.build(symbol, family, params), U2.build(symbol, family, params)
    sd = U1.sd[symbol]
    k = int(np.searchsorted(sd.ord, boundary_ord, side="right"))
    if a is None or b is None:
        return a is None and b is None
    return (np.array_equal(a.d[:k], b.d[:k]) and np.array_equal(a.cond[:k], b.cond[:k])
            and (a.s == b.s or a.s > k))


@pytest.fixture(scope="module")
def U_perturbed(bars):
    boundary = sg.SESSION_INDEX[dt.date(2022, 6, 30)]
    return perturbed(bars, boundary), boundary


def test_every_family_signal_is_prefix_invariant_to_future_bars(bars, U, U_perturbed):
    pb, boundary = U_perturbed
    U2 = sg.Universe(pb)
    checked = 0
    for c in first_configs(3):
        for sym in ("SPY", "X03", "X04"):
            if c["scope"] == "universe":
                assert prefix_invariant(U, U2, sym, c["family"], c["params"], boundary), (c["family"], sym)
                checked += 1
                break
            assert prefix_invariant(U, U2, sym, c["family"], c["params"], boundary), (c["family"], c["params"], sym)
            checked += 1
    assert checked >= 40
    assert any(U.build("X03", c["family"], c["params"]) is not None for c in first_configs(3) if c["family"] == "S02")


def test_prefix_invariance_check_detects_a_leaky_signal(bars, U, U_perturbed):
    pb, boundary = U_perturbed
    U2 = sg.Universe(pb)

    class Leaky:
        def __init__(self, uni):
            self.uni, self.sd = uni, uni.sd

        def build(self, symbol, *_):
            sd = self.uni.sd[symbol]
            d = np.zeros(sd.n, bool)
            d[:-1] = sd.c[1:] > sd.c[:-1]  # looks at bar t+1
            return sg.Sig(d, d, 1)

    assert prefix_invariant(Leaky(U), Leaky(U2), "X03", "S01", {}, boundary) is False


def test_s18_thresholds_and_s19_fits_use_only_rows_before_the_fold(bars, monkeypatch):
    start_2023 = sg.SESSION_INDEX[next(d for d in SESSIONS if d.year == 2023)]
    pb = perturbed(bars, start_2023 - 1)  # every bar from the 2023 fold start onward is changed
    base, pert = sg.Universe(bars), sg.Universe(pb)
    feat = "ret_20" if "ret_20" in base.features["X03"] else sorted(base.features["X03"])[0]
    p18 = {"feature": feat, "q": 0.7, "side": "ge", "hold": 5}
    p19 = {"feature": feat, "label_horizon": 5, "p_entry": 0.5}
    calls = {}

    import mqk_research.ml.model_logreg as ml
    real = ml.fit_logreg_deterministic

    def record(X, y, **kw):
        calls.setdefault(id(record.cur), []).append((np.array(X, copy=True), np.array(y, copy=True)))
        return real(X, y, **kw)

    monkeypatch.setattr(ml, "fit_logreg_deterministic", record)
    out = {}
    for name, uni in (("base", base), ("pert", pert)):
        record.cur = uni
        uni.build("X03", "S18", p18)
        uni.build("X03", "S19", p19)
        out[name] = calls[id(uni)]
    # thresholds of every fold up to 2023 are bit-identical although the fold-year bars differ
    for y in range(2017, 2024):
        k = ("s18thr", feat, y)
        a, b = base.sd["X03"]._memo.get(k), pert.sd["X03"]._memo.get(k)
        assert (a is None) == (b is None) and (a is None or np.array_equal(a, b)), y
    thr24 = [np.array_equal(base.sd["X03"]._memo[("s18thr", feat, 2024)], pert.sd["X03"]._memo[("s18thr", feat, 2024)])]
    assert thr24 == [False]  # fixture sensitivity: the 2024 fold (which sees the perturbed rows) does differ
    # S19: every fit whose training window ends before the 2023 purge boundary is identical (labels purged)
    lim = start_2023 - 5
    same = [(len(a[0]) <= lim) for a in out["base"]]
    assert sum(same) >= 3
    for (xa, ya), (xb, yb) in zip(out["base"], out["pert"]):
        if len(xa) <= lim:
            assert np.array_equal(xa, xb) and np.array_equal(ya, yb)
    # the purge matters: the label of the last un-purged row would read a perturbed bar
    sdb, sdp = base.sd["X03"], pert.sd["X03"]
    assert (sdb.c[start_2023 + 2] / sdb.c[start_2023 - 3]) != (sdp.c[start_2023 + 2] / sdp.c[start_2023 - 3])


def test_s19_is_exactly_one_feature_per_model():
    assert all(set(c["params"]) == {"feature", "label_horizon", "p_entry"} for c in ss.build_configs() if c["family"] == "S19")
    assert all(isinstance(c["params"]["feature"], str) for c in ss.build_configs() if c["family"] == "S19")


@pytest.mark.parametrize("bad", [
    {"feature": ["ret_20", "ret_5"], "label_horizon": 5, "p_entry": 0.5},
    {"feature": "ret_20", "feature2": "ret_5", "label_horizon": 5, "p_entry": 0.5},
])
def test_s19_refuses_a_model_given_two_features(U, bad):
    with pytest.raises(ValueError, match="exactly one feature"):
        U.s19("X03", bad)


def test_universe_builder_fails_closed_when_snapshot_claims_point_in_time(monkeypatch):
    import mqk_research.universe.snapshot as snap_mod
    real = snap_mod.build_current_enabled_equity_registry_snapshot

    def lying(*a, **k):
        real_snap = real(*a, **k)

        class Wrap:
            def to_json_dict(self_inner):
                return {**real_snap.to_json_dict(), "point_in_time_membership": True}
        return Wrap()

    monkeypatch.setattr(snap_mod, "build_current_enabled_equity_registry_snapshot", lying)
    with pytest.raises(RuntimeError, match="fail closed"):
        ss.build_universe()


def test_s14_calendar_membership_comes_from_authority_not_prices(bars, U, U_perturbed):
    pb, boundary = U_perturbed
    c = next(c for c in ss.build_configs() if c["family"] == "S14")
    a = U.build("X03", c["family"], c["params"])
    b = sg.Universe(pb).build("X03", c["family"], c["params"])
    assert np.array_equal(a.d, b.d) and a.d.any()  # whole-array equality: calendar d ignores prices entirely


# ------------------------------------------------------------------------------------- conditional edges

def _fake_sd(closes, years=None):
    n = len(closes)
    bars = pd.DataFrame({"symbol": "F", "end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[:n]],
                         "open": closes, "high": closes, "low": closes, "close": closes, "volume": np.ones(n)})
    sd = sg.SymbolData("F", bars)
    sd.regime = np.array([i % 2 for i in range(n)], np.int8)
    return sd


def test_conditional_stats_hand_computed_and_never_pnl():
    c = np.array([100.0, 101, 103, 102, 106, 105, 110, 108, 107, 111, 115, 113])
    sd = _fake_sd(c)
    cond = np.zeros(len(c), bool)
    cond[[1, 3, 5]] = True
    q = cd.conditional_stats([(sd, sg.Sig(cond, cond, 0))], (1, 2))
    r1 = q[1]
    fwd1 = c[1:] / c[:-1] - 1
    sel = fwd1[[1, 3, 5]]
    assert r1["n"] == 3 and r1["mean"] == pytest.approx(sel.mean()) and r1["median"] == pytest.approx(np.median(sel))
    assert r1["std"] == pytest.approx(sel.std(ddof=1)) and r1["positive_freq"] == pytest.approx((sel > 0).mean())
    assert r1["uncond_mean"] == pytest.approx(fwd1.mean()) and r1["effect"] == pytest.approx(sel.mean() - fwd1.mean())
    assert r1["tiny_sample"] is True and sum(v[0] for v in r1["year"].values()) == 3
    assert sum(v[0] for v in r1["regime"].values()) == 3
    assert "pnl" not in json.dumps(r1).lower()
    assert q[2]["n"] == 3  # horizon 2: t=1,3,5 all have t+2 available
    last = np.zeros(len(c), bool)
    last[-1] = True
    assert cd.conditional_stats([(sd, sg.Sig(last, last, 0))], (1,))[1] is None  # no forward window => no record


def test_non_positive_effect_is_not_positive():
    base = {"effect": 0.0}
    assert not cd.is_positive(base) and not cd.is_positive({"effect": -1e-9}) and not cd.is_positive({"effect": float("nan")})
    assert cd.is_positive({"effect": 1e-12}) and not cd.is_positive(None)




# ------------------------------------------------------------------------------------------- store (bulk)

def _row(i, exp="e"):
    return {"trial_id": f"t{i}", "experiment_id": exp, "hypothesis_id": "h", "strategy_id": f"s{i}", "protocol_id": "p",
            "identity": {"i": i}}


def test_bulk_store_semantics(tmp_path):
    st = ResearchResultStore(tmp_path / "r.sqlite")
    st.register_hypothesis(hypothesis_id="h", experiment_id="e", hypothesis_text="x")
    assert st.register_trials_bulk([_row(i) for i in range(5)]) == 5
    assert st.register_trials_bulk([_row(i) for i in range(5)]) == 0  # idempotent
    bad = _row(2)
    bad["identity"] = {"i": 99}
    with pytest.raises(RuntimeError, match="collision"):
        st.register_trials_bulk([_row(7), bad])
    assert set(st.trial_attempt_digest("e")) == {f"t{i}" for i in range(5)}  # whole call aborted, t7 absent
    assert all(v["attempts"] == 0 for v in st.trial_attempt_digest("e").values())
    started = st.begin_attempts_bulk(["t0", "t1"], origin="o", metadata={"k": 1})
    assert [i for _a, i in started] == [1, 1]
    st.finalize_attempts_bulk([{"attempt_id": started[0][0], "status": "succeeded"},
                               {"attempt_id": started[1][0], "status": "failed", "failure_reason": "x"}])
    with pytest.raises(RuntimeError, match="already terminal"):
        st.finalize_attempts_bulk([{"attempt_id": started[0][0], "status": "succeeded"}])
    again = st.begin_attempts_bulk(["t1"], origin="o")
    assert again[0][1] == 2 and again[0][0] == "t1:att0002"
    d = st.trial_attempt_digest("e")
    assert d["t0"] == {"attempts": 1, "started": 0, "succeeded": 1, "failed": 0, "blocked": 0}
    assert d["t1"]["failed"] == 1 and d["t1"]["started"] == 1 and d["t2"]["attempts"] == 0
    with pytest.raises(KeyError):
        st.begin_attempts_bulk(["t1", "missing"])
    assert st.trial_attempt_digest("e")["t1"]["attempts"] == 2  # aborted call left no partial attempts
    with pytest.raises(ValueError):
        st.finalize_attempts_bulk([{"attempt_id": again[0][0], "status": "started"}])


# ------------------------------------------------------------- census gate / execution / resume / registry

@pytest.fixture(scope="module")
def mini(U):
    prot = ss.build_protocol()
    full_uni = json.loads((EXP / "ALPHA_CENSUS_UNIVERSE_V1.json").read_text())
    syms = list(U.symbols)
    uni = {**full_uni, "symbols": syms}
    cfgs = first_configs(2)
    space = {"search_space_id": "mini", "universe_id": "u" * 32, "partitions_id": "p" * 32, "protocol_id": "q" * 32,
             "configs": cfgs}
    ids = {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}
    index = {c["config_id"]: i for i, c in enumerate(cfgs)}
    cells = [(index[c["config_id"]], c, s, t) for c, s, t in ss.iter_cells(cfgs, syms, ids)]
    space["population_root_sha256"] = ss.population_root(cfgs, syms, ids)
    meta = {s: {"disposition": ce.DATA_PRESENT, "rows": len(U.sd[s].c), "data_short_history": s == syms[-1],
                "data_quality_caveat": False} for s in syms}
    return {"space": space, "cells": cells, "meta": meta, "uni": uni, "prot": prot}


def _fresh_store(tmp_path, mini, *, register=True):
    st = ResearchResultStore(tmp_path / "registry.sqlite")
    if register:
        ce.register_population(st, mini["space"], mini["cells"])
    return st


def test_gate_accepts_exact_frozen_population_with_zero_attempts(tmp_path, mini):
    st = _fresh_store(tmp_path, mini)
    r = ce.require_frozen_population(st, mini["space"], mini["cells"], allow_attempts=False)
    assert r["registered"] == len(mini["cells"]) > 100 and r["attempts"] == 0


def test_gate_refuses_missing_extra_duplicate_and_unfrozen(tmp_path, mini):
    cells = mini["cells"]
    # M03: one registered cell missing
    st = ResearchResultStore(tmp_path / "a.sqlite")
    ce.register_population(st, mini["space"], cells[:-1])
    with pytest.raises(ce.GateRefusal, match="missing=1"):
        ce.require_frozen_population(st, mini["space"], cells, allow_attempts=False)
    # M04: an undeclared extra cell is registered
    st = _fresh_store(tmp_path, mini)
    st.register_trials_bulk([{"trial_id": "ace1-extra", "experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": "alpha_edge_census_01:S01",
                              "strategy_id": "x", "protocol_id": "q", "identity": {"x": 1}}])
    with pytest.raises(ce.GateRefusal, match="extra=1"):
        ce.require_frozen_population(st, mini["space"], cells, allow_attempts=True)
    # duplicate expected cell
    with pytest.raises(ce.GateRefusal, match="duplicate"):
        ce.freeze_record(mini["space"], cells + [cells[0]])
    # attempt before population freeze: trials registered, freeze marker absent
    st = ResearchResultStore(tmp_path / "c.sqlite")
    tmp = ResearchResultStore(tmp_path / "scratch.sqlite")
    ce.register_population(tmp, mini["space"], cells)
    st.register_hypothesis(hypothesis_id="alpha_edge_census_01:S01", experiment_id=ss.EXPERIMENT_ID, hypothesis_text="x")
    for fam in {c[1]["family"] for c in cells}:
        st.register_hypothesis(hypothesis_id=ss.hypothesis_id(fam), experiment_id=ss.EXPERIMENT_ID, hypothesis_text="x")
    ids = {k: mini["space"][k] for k in ("universe_id", "partitions_id", "protocol_id")}
    st.register_trials_bulk([{"trial_id": t, "experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": ss.hypothesis_id(c["family"]),
                              "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": ids["protocol_id"],
                              "identity": ss.trial_identity(c, s, ids)} for _i, c, s, t in cells])
    with pytest.raises(ce.GateRefusal, match="freeze marker"):
        ce.require_frozen_population(st, mini["space"], cells, allow_attempts=True)
    # attempts != 0 at freeze time
    (tmp_path / "d").mkdir()
    st = _fresh_store(tmp_path / "d", mini)
    st.begin_attempts_bulk([cells[0][3]], origin="x")
    with pytest.raises(ce.GateRefusal, match="attempts already exist"):
        ce.require_frozen_population(st, mini["space"], cells, allow_attempts=False)
    ce.require_frozen_population(st, mini["space"], cells, allow_attempts=True)


def test_run_chunks_refuses_unregistered_population(tmp_path, mini, U):
    st = ResearchResultStore(tmp_path / "e.sqlite")
    with pytest.raises(ce.GateRefusal):
        ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path / "out", chunk_size=10)
    assert not (tmp_path / "out" / "chunks").exists() or not list((tmp_path / "out" / "chunks").iterdir())


def _read(out):
    lines = []
    for p in sorted((Path(out) / "chunks").glob("chunk_*.jsonl")):
        lines += p.read_text(encoding="utf-8").splitlines()
    return lines


@pytest.fixture(scope="module")
def reference_run(tmp_path_factory, mini, U):
    root = tmp_path_factory.mktemp("ref")
    st = _fresh_store(root, mini)
    ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], root, chunk_size=13)
    return root, st


def test_chunk_size_does_not_change_cell_economics_or_denominator(tmp_path, mini, U, reference_run):
    root, st = reference_run
    ref = _read(root)
    assert len(ref) == len(mini["cells"])
    st2 = _fresh_store(tmp_path, mini)
    ce.run_chunks(st2, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=50)
    assert _read(tmp_path) == ref
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(v["attempts"] == 1 and v["succeeded"] == 1 for v in d.values()) and len(d) == len(mini["cells"])


def test_cell_lines_follow_manifest_order_and_contain_real_evaluations(mini, reference_run):
    root, _ = reference_run
    rows = [json.loads(x) for x in _read(root)]
    assert [r["t"] for r in rows] == [c[3] for c in mini["cells"]]
    ev = [r for r in rows if r["d"] == "EVALUABLE"]
    assert len(ev) > 100 and any(r["m"]["trade_count"] > 0 for r in ev)
    assert any(r["m"]["net_alpha_usd"] > 0 for r in ev), "fixture must contain positive-alpha cells (false-positive guard)"
    assert any(isinstance(q, dict) for r in ev for q in r["q"].values())


def test_interrupted_chunk_is_retried_as_new_attempt_with_identical_economics(tmp_path, mini, U, reference_run, monkeypatch):
    ref = _read(reference_run[0])
    st = _fresh_store(tmp_path, mini)
    real = ce.evaluate_cell
    n = {"i": 0}

    def flaky(*a, **k):
        n["i"] += 1
        if n["i"] == 20:
            raise RuntimeError("simulated infrastructure fault")
        return real(*a, **k)

    monkeypatch.setattr(ce, "evaluate_cell", flaky)
    with pytest.raises(RuntimeError, match="simulated"):
        ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13)
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    failed = [t for t, v in d.items() if v["failed"]]
    assert len(failed) == 13 and all(d[t]["started"] == 0 for t in d)  # the whole interrupted chunk is closed as failed
    assert len(list((tmp_path / "chunks").glob("chunk_*.jsonl"))) == 1
    monkeypatch.setattr(ce, "evaluate_cell", real)
    ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13)
    assert _read(tmp_path) == ref
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert len(d) == len(mini["cells"])  # same trials: retry created no new trial identity (M15)
    assert all(d[t]["attempts"] == 2 and d[t]["failed"] == 1 and d[t]["succeeded"] == 1 for t in failed)
    assert all(v["succeeded"] == 1 for v in d.values())


def test_crash_leftover_started_attempts_are_finalized_then_retried(tmp_path, mini, U, reference_run):
    st = _fresh_store(tmp_path, mini)
    part = mini["cells"][:13]
    st.begin_attempts_bulk([c[3] for c in part], origin="crashed")  # process died with attempts left 'started'
    res = ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13)
    assert res["chunks_run"] == -(-len(mini["cells"]) // 13)
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(d[c[3]]["failed"] == 1 and d[c[3]]["succeeded"] == 1 and d[c[3]]["started"] == 0 for c in part)
    assert _read(tmp_path) == _read(reference_run[0])


def test_resume_skips_terminal_chunks_and_is_idempotent(tmp_path, mini, U, reference_run):
    st = _fresh_store(tmp_path, mini)
    r1 = ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13, max_chunks=3)
    assert r1["chunks_run"] == 3
    before = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    r2 = ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13)
    assert r2["chunks_skipped_terminal"] == 3
    r3 = ce.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], tmp_path, chunk_size=13)
    assert r3["chunks_run"] == 0
    after = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(v["attempts"] == 1 for v in after.values()) and after == before | {t: after[t] for t in after}
    assert _read(tmp_path) == _read(reference_run[0])


def test_non_evaluable_symbol_stays_in_population(mini, U):
    meta = copy.deepcopy(mini["meta"])
    meta["X02"] = {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR"}
    cfg = next(c for c in mini["space"]["configs"] if c["scope"] == "symbol")
    r = ce.evaluate_cell(U, cfg, "X02", meta)
    assert r["d"] == "NON_EVALUABLE_DATA_UNAVAILABLE_PROVIDER_ERROR" and r["m"] is None and r["q"] is None


def test_universe_cell_pools_members_and_s02_defined_only_with_cross_section(mini, U):
    cfg = next(c for c in mini["space"]["configs"] if c["family"] == "S02")
    r = ce.evaluate_cell(U, cfg, None, mini["meta"])
    assert r["d"] == "EVALUABLE" and r["m"]["members"] >= 2 and len(r["m"]["member_symbols"]) == r["m"]["members"]
    assert r["m"]["window_bars"] > 0


@pytest.fixture(scope="module")
def registry_out(mini, reference_run):
    root, _ = reference_run
    import shutil
    out = root.parent / "reg_out"
    shutil.copytree(root, out, ignore=shutil.ignore_patterns("*.sqlite"))
    prot = {**mini["prot"], "chunking": {"cells_per_chunk": 13}}
    summary = er.build_registry({**mini["space"]}, mini["uni"], mini["cells"], mini["meta"], prot, out)
    return out, summary, prot


def _edges(out):
    return [json.loads(x) for x in (Path(out) / "edge_registry_v1.jsonl").read_text(encoding="utf-8").splitlines()]


def test_registry_records_every_positive_observation_and_nothing_else(registry_out, mini, reference_run):
    out, summary, _ = registry_out
    rows = [json.loads(x) for x in _read(reference_run[0])]
    exp_s = {r["t"] for r in rows if r["m"] and r["m"]["net_alpha_usd"] > 0}
    exp_c = {(r["t"], int(h)) for r in rows if r["m"] for h, q in r["q"].items() if isinstance(q, dict)}
    edges = _edges(out)
    got_s = {e["trial_id"] for e in edges if e["kind"] == "STRATEGY_EDGE"}
    got_c = {(e["trial_id"], e["horizon"]) for e in edges if e["kind"] == "CONDITIONAL_EDGE"}
    assert got_s == exp_s and got_c == exp_c and len(exp_s) > 0 and len(exp_c) > 0
    assert len(edges) == len(got_s) + len(got_c) and len({e["edge_id"] for e in edges}) == len(edges)
    for e in edges:
        key = e["metrics"]["net_alpha_usd"] if e["kind"] == "STRATEGY_EDGE" else e["stats"]["effect"]
        assert key > 0 and np.isfinite(key)
    assert summary["strategy_edges"] == len(got_s) and summary["conditional_edges"] == len(got_c)


def test_conditional_edges_are_non_executable_labels_never_pnl(registry_out):
    out, _summary, _ = registry_out
    cond = [e for e in _edges(out) if e["kind"] == "CONDITIONAL_EDGE"]
    assert cond
    for e in cond:
        assert e["executable_pnl"] is False and "metrics" not in e
        assert not any(("pnl" in k or "alpha" in k or "net_" in k) for k in e["stats"])
    assert all("executable_pnl" not in e or e["executable_pnl"] is False for e in _edges(out))


def test_every_registry_record_is_not_validated_with_no_promotion_authority_and_labels(registry_out):
    out, summary, _ = registry_out
    for e in _edges(out):
        assert e["VALIDATION_STATUS"] == "NOT_VALIDATED" and e["PROMOTION_AUTHORITY"] == "NONE"
        assert e["label"] == "DISCOVERED / NOT VALIDATED" and e["partition"] == "DISCOVERY_PRE_2025"
        assert e["point_in_time_membership"] is False
        assert e["universe_source_kind"] == "current_enabled_equity_registry_snapshot_v1"
        assert e["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"
        assert "survivorship_caveat" in e["flags"]
        assert e["kind"] != "CONDITIONAL_EDGE" or e["executable_pnl"] is False
    assert summary["JUDGE_STATUS"] == "DEFERRED_FULL_POPULATION" and summary["PROMOTION_AUTHORITY"] == "NONE"
    assert "label" in summary and summary["label"] == "DISCOVERED / NOT VALIDATED"


def test_search_ledger_holds_the_full_denominator_not_only_winners(registry_out, mini):
    out, summary, _ = registry_out
    led = [json.loads(x) for x in (out / "search_ledger_v1.jsonl").read_text(encoding="utf-8").splitlines()]
    assert [r["t"] for r in led] == [c[3] for c in mini["cells"]]
    assert len(led) == summary["cells_total"] == len(mini["cells"]) > summary["strategy_edges"] > 0
    assert any(r["net_alpha_usd"] <= 0 for r in led if "net_alpha_usd" in r)  # non-winners present (M10)
    assert sum(summary["dispositions"].values()) == len(mini["cells"])


def test_registry_is_deterministic_and_hashes_bind_files(registry_out, mini):
    out, summary, prot = registry_out
    import hashlib
    assert hashlib.sha256((out / "edge_registry_v1.jsonl").read_bytes()).hexdigest() == summary["edge_registry_sha256"]
    assert hashlib.sha256((out / "search_ledger_v1.jsonl").read_bytes()).hexdigest() == summary["search_ledger_sha256"]
    again = er.build_registry({**mini["space"]}, mini["uni"], mini["cells"], mini["meta"], prot, out)
    assert again == summary


def test_neighborhood_flags_never_delete_island_edges(registry_out):
    out, summary, _ = registry_out
    edges = _edges(out)
    islands = [e for e in edges if "parameter_island" in e["flags"]]
    assert summary["parameter_island_edges"] == len(islands)
    for e in islands:
        assert e["neighborhood"]["n_neighbors"] >= 1 and e["neighborhood"]["positive_share"] < 0.25
    assert all(e["neighborhood"]["n_neighbors"] == 0 or e["neighborhood"]["positive_share"] is not None for e in edges)
    assert len(edges) == summary["strategy_edges"] + summary["conditional_edges"]  # flags annotate, never filter


def test_neighbor_map_adjacent_numeric_only():
    cfgs = [{"family": "F", "params": {"a": a, "b": b, "m": "x"}} for a in (1, 2, 4) for b in (10, 20)]
    nb = er.neighbor_map(cfgs)
    idx = {(c["params"]["a"], c["params"]["b"]): i for i, c in enumerate(cfgs)}
    assert set(nb[idx[(2, 10)]]) == {idx[(1, 10)], idx[(4, 10)], idx[(2, 20)]}
    assert set(nb[idx[(1, 10)]]) == {idx[(2, 10)], idx[(1, 20)]}  # diagonal moves are not neighbours
    other = [{"family": "G", "params": {"a": 2}}, {"family": "F", "params": {"a": 3}}]
    assert er.neighbor_map(other) == [[], []]  # different families never neighbour


def test_replication_classes_present_and_symbol_specific_flagged(registry_out):
    out, summary, _ = registry_out
    for e in _edges(out):
        if e["scope"] == "UNIVERSE":
            assert e["replication"]["class"] == "NOT_APPLICABLE_UNIVERSE_SCOPE"
        else:
            r = e["replication"]
            assert r["class"] in ("SYMBOL_SPECIFIC", "CLUSTER_REPLICATED", "BROADLY_REPLICATED")
            assert (r["class"] == "SYMBOL_SPECIFIC") == ("single_symbol" in e["flags"]) == (r["n_positive_symbols"] == 1)


def test_registry_refuses_tampered_or_missing_chunk(tmp_path, mini, U, reference_run):
    import shutil
    root = tmp_path / "t"
    shutil.copytree(reference_run[0], root, ignore=shutil.ignore_patterns("*.sqlite"))
    prot = {**mini["prot"], "chunking": {"cells_per_chunk": 13}}
    p0 = er.chunk_path(root, 0)
    lines = p0.read_text(encoding="utf-8").splitlines()
    p0.write_text("\n".join(lines[1:] + lines[:1]) + "\n", encoding="utf-8")
    with pytest.raises(er.RegistryRefusal, match="not the manifest cell"):
        er.build_registry(mini["space"], mini["uni"], mini["cells"], mini["meta"], prot, root)
    p0.write_text("\n".join(lines[:-1]) + "\n", encoding="utf-8")
    with pytest.raises(er.RegistryRefusal, match="line count"):
        er.build_registry(mini["space"], mini["uni"], mini["cells"], mini["meta"], prot, root)
    p0.unlink()
    with pytest.raises(er.RegistryRefusal, match="missing chunk"):
        er.build_registry(mini["space"], mini["uni"], mini["cells"], mini["meta"], prot, root)


def _fake_bars_dir(tmp_path, symbols, absent=()):
    for sym in symbols:
        d = tmp_path / sym
        d.mkdir()
        status = {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR" if sym in absent else ce.DATA_PRESENT}
        (d / "status.json").write_text(json.dumps(status), encoding="utf-8")
        if sym not in absent:
            (d / "research_bars_provenance.json").write_text(json.dumps({"sym": sym}), encoding="utf-8")
            (d / "corporate_actions_provenance.json").write_text(json.dumps({"ca": sym}), encoding="utf-8")


def test_bars_manifest_binds_hashes_keeps_unavailable_symbols_and_load_refuses_drift(tmp_path, monkeypatch, bars):
    syms = ["SPY", "X00", "X01"]
    uni = {"symbols": syms}
    _fake_bars_dir(tmp_path, syms, absent=("X01",))
    state = {"hash": "h1"}

    def fake_load(sym_dir):
        b = bars[sym_dir.name].copy()
        return b, {"artifact_sha256": "a-" + sym_dir.name, "canonical_semantic_bars_hash": state["hash"] + sym_dir.name,
                   "canonical_pricing_bars_hash": "p"}

    monkeypatch.setattr(ce, "load_symbol_bars", fake_load)
    m = ce.build_bars_manifest(uni, tmp_path)
    assert set(m["symbols"]) == set(syms)  # the unavailable symbol is not dropped (M11)
    assert m["symbols"]["X01"] == {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR"}
    assert m["symbols"]["SPY"]["rows"] == len(bars["SPY"]) and m["symbols"]["SPY"]["provenance_file_sha256"]
    assert m["request_contract"]["feed"] == "sip"
    assert ce.load_bars(uni, tmp_path, m).keys() == {"SPY", "X00"}
    state["hash"] = "h2"  # same bytes path, different semantic identity
    with pytest.raises(ce.GateRefusal, match="differ"):
        ce.load_bars(uni, tmp_path, m)
    m2 = ce.build_bars_manifest(uni, tmp_path)
    assert m2["manifest_sha256"] != m["manifest_sha256"]


def test_symbol_meta_flags_short_history_and_zero_volume_caveat():
    prot = ss.build_protocol()
    man = {"symbols": {"A": {"disposition": ce.DATA_PRESENT, "rows": 1499, "zero_volume_bars": 20},
                       "B": {"disposition": ce.DATA_PRESENT, "rows": 1500, "zero_volume_bars": 19},
                       "C": {"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION"}}}
    meta = ce.symbol_meta(man, prot)
    assert meta["A"]["data_short_history"] and meta["A"]["data_quality_caveat"]
    assert not meta["B"]["data_short_history"] and not meta["B"]["data_quality_caveat"]
    assert meta["C"] == {"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION"}
