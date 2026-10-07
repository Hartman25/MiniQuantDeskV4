"""Census-02 predeclaration infrastructure: grammar/identity, signed causal execution, borrow capability, fences,
freeze guard and denominator guards. Synthetic fixtures only; no real Census-02 result is read or produced."""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

EXP2 = Path(__file__).resolve().parents[1] / "experiments" / "alpha_edge_census_02"
EXP1 = EXP2.parent / "alpha_edge_census_01"
sys.path.insert(0, str(EXP2))

import c2_borrow as bw  # noqa: E402
import c2_factors as fx  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_signals as sg2  # noqa: E402
import c2_simulate as sim2  # noqa: E402
import search_space as ss1  # noqa: E402
import signals as sg1  # noqa: E402
import simulate as sm1  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

SESSIONS = sg1.SESSIONS
ASSUME = {"etf_short_scope": ["IWM", "SPY"], "annual_borrow_fee_bps": 50, "availability": bw.ASSUMPTION_FIXED["availability"],
          "recall": "NONE_ASSUMED", "short_rebate": "ZERO"}
IDS = {"universe_id": "u" * 32, "partitions_id": "p" * 32, "protocol_id": "q" * 32}


def synth_bars(symbol, seed, *, phi=-0.15, mu=0.0003, sig=0.012, price0=50.0, n=None, gap=0.002, bursts=False):
    rng = np.random.default_rng(seed)
    n = n or len(SESSIONS)
    r = np.zeros(n)
    eps = rng.standard_normal(n)
    if bursts:                                             # volatility regimes so expansion/gap events actually occur
        for a in range(40, n - 20, 90):
            eps[a:a + 12] *= 3.5
    for i in range(1, n):
        r[i] = mu + phi * r[i - 1] + sig * eps[i]
    close = np.round(price0 * np.exp(np.cumsum(r)), 2)
    open_ = np.round(np.concatenate([[close[0]], close[:-1]]) * (1 + gap * rng.standard_normal(n)), 2)
    hi = np.maximum(np.round(np.maximum(open_, close) * (1 + np.abs(rng.standard_normal(n)) * 0.004), 2), np.maximum(open_, close))
    lo = np.minimum(np.round(np.minimum(open_, close) * (1 - np.abs(rng.standard_normal(n)) * 0.004), 2), np.minimum(open_, close))
    vol = np.round(1e6 * (1 + rng.random(n)), 0)
    ts = [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[:n]]
    return pd.DataFrame({"symbol": symbol, "end_ts": ts, "open": open_, "high": hi, "low": lo, "close": close, "volume": vol})


@pytest.fixture(scope="module")
def bars():
    return synth_bars("TST", 11)


@pytest.fixture(scope="module")
def gappy():
    return synth_bars("GAP", 23, gap=0.02, bursts=True)


@pytest.fixture(scope="module")
def sd(bars):
    s = sg2.build_symbol_data("TST", bars)
    s.regime = np.ones(s.n, np.int8)
    return s


def micros(*xs):
    return [np.array([int(round(v * 1e6)) for v in x], np.int64) for x in xs]


# ------------------------------------------------------------------------------ scalar reference (independent)
def reference(hm, lm, cm, d, fee_bps=None, commission=10.0, slip=5):
    n = len(cm)
    pos_after = [0] * n
    qty = 0
    net = [0.0] * n
    q_prev = 0
    for b in range(n):
        h = int(d[b - 1]) if b else 0
        q_before = q_prev
        if h != 0 and (b == 0 or int(d[b - 2]) != h if b >= 2 else True):
            q_new = h * (10_000_000_000 // int(cm[b - 1]))
        elif h != 0:
            q_new = q_before
        else:
            q_new = 0
        spread = (int(hm[b]) - int(lm[b])) * 10_000 // int(cm[b])
        eff = slip + spread * 0
        buy = int(hm[b]) + int(hm[b]) * eff // 10_000
        sell = max(int(lm[b]) - int(lm[b]) * eff // 10_000, 0)
        gross = q_before * (int(cm[b]) - int(cm[b - 1])) if b else 0
        delta = q_new - q_before
        adv = abs(delta) * ((buy - int(cm[b])) if delta > 0 else (int(cm[b]) - sell)) if delta else 0
        px = buy if delta > 0 else sell
        comm = abs(delta) * px * (commission / 10_000.0) if delta else 0.0
        borrow = (-q_before * int(cm[b - 1]) * (fee_bps / 10_000.0 / 252)) if (fee_bps and q_before < 0 and b) else 0.0
        net[b] = (gross - adv - comm - borrow) / 1e6
        q_prev = q_new
    return np.array(net)


# ====================================================================================== grammar / identity
def test_grammar_counts_and_arithmetic():
    cfgs = gr.build_configs()
    assert len(cfgs) == 470 and len(gr.build_configs("H")) == 430
    assert {c["family"] for c in cfgs} == set(gr.EXPECTED_H_COUNTS) | set(gr.EXPECTED_L_COUNTS)
    assert len({c["config_id"] for c in cfgs}) == 470
    a = gr.candidate_arithmetic(20)
    assert (a["strategy_trials"], a["conditional_factors"], a["complement_tagged_configs"]) == (9_400, 1_075, 56)
    assert gr.candidate_arithmetic(88, "H")["strategy_trials"] == 37_840
    # REGISTER_ALL_TAG_COMPLEMENTS keeps the whole population; EXCLUDE removes 56 configs AND the 28 SH01-03 conditions
    keep = gr.candidate_arithmetic(20)
    drop = gr.candidate_arithmetic(20, complements_excluded=True)
    assert (keep["configs"], keep["short_conditions"], keep["conditional_factors"]) == (470, 215, 1075)
    assert (drop["configs"], drop["short_conditions"], drop["conditional_factors"], drop["strategy_trials"]) == (414, 187, 935, 8_280)
    assert drop["complement_tagged_configs"] == 0
    h_drop = gr.candidate_arithmetic(20, "H", complements_excluded=True)
    assert (h_drop["configs"], h_drop["short_conditions"], h_drop["conditional_factors"]) == (402, 187, 935)


def test_complement_exclusion_removes_exactly_the_complement_families_and_their_conditions():
    full_c, full_k = gr.selected_population("H+L", False)
    cut_c, cut_k = gr.selected_population("H+L", True)
    assert {c["family"] for c in full_c} - {c["family"] for c in cut_c} == set(gr.COMPLEMENT_FAMILIES)
    assert {k["family"] for k in full_k} - {k["family"] for k in cut_k} == {"SH01", "SH02", "SH03"}
    assert len(full_k) - len(cut_k) == 8 + 6 + 14
    assert {k["condition_id"] for k in cut_k} <= {k["condition_id"] for k in full_k}
    assert not any(k["family"].startswith("LS") for k in full_k)                      # LS mints no factor of its own
    with pytest.raises(gr.GrammarRefusal):                                           # stale arithmetic: 215 conditions under exclusion
        gr.build_conditions(cut_c, complements_excluded=False)
    with pytest.raises(gr.GrammarRefusal):
        gr.build_conditions(full_c, complements_excluded=True)


def test_no_collision_with_census01_and_sides_are_not_long():
    c1 = {c["config_id"] for c in ss1.build_configs()}
    assert not c1 & {c["config_id"] for c in gr.build_configs()}
    assert all(c["side"] in ("short", "long_short") for c in gr.build_configs())
    # identical params on a mirrored family never share an identity with the long coordinate
    ident_long = {"family": "S02", "params": {"sma": 50}}
    assert gr.config_id("SH02", {"sma": 50}) != ss1.config_id(ident_long["family"], ident_long["params"])


def test_unimplemented_tier_is_refused():
    for bad in ("H+L+X", "L", "X", ""):
        with pytest.raises(gr.GrammarRefusal):
            gr.build_configs(bad)


def test_forced_census01_id_collision_is_refused(monkeypatch):
    first = gr.build_configs()[0]["config_id"]
    monkeypatch.setattr(gr.ss1, "build_configs", lambda: [{"config_id": first}])
    with pytest.raises(gr.GrammarRefusal):
        gr.build_configs()


def test_grammar_drift_is_refused():
    cfgs = gr.build_configs()
    with pytest.raises(gr.GrammarRefusal):
        gr.assert_grammar_authority(cfgs[:-1])
    extra = cfgs + [{"config_id": "x", "family": "SH99", "side": "short", "params": {}}]
    with pytest.raises((gr.GrammarRefusal, KeyError)):
        gr.assert_grammar_authority(extra)


def test_conditions_exact_and_execution_only_keys_never_mint_a_condition():
    cfgs = gr.build_configs()
    conds = gr.build_conditions(cfgs)
    assert len(conds) == 215 and len({c["condition_id"] for c in conds}) == 215
    for c in conds:
        assert not set(c["params"]) & set(gr.EXECUTION_ONLY_KEYS.get(c["family"], ()))
    with pytest.raises(gr.GrammarRefusal):
        gr.condition_params("SH07", {"rise_sessions": 1, "atr_window": 14, "mult": 1.0, "hold": 1, "trend": "none", "bogus": 1})


def test_execution_only_params_change_d_but_never_cond(sd):
    pairs = {"SH04": ({"entry": 50, "exit": 10}, {"entry": 50, "exit": 20}),
             "SH05": ({"period": 5, "entry_above": 70, "exit_below": 50, "trend": "none"},
                      {"period": 5, "entry_above": 70, "exit_below": 30, "trend": "none"}),
             "SH07": ({"rise_sessions": 1, "atr_window": 14, "mult": 1.0, "hold": 1, "trend": "none"},
                      {"rise_sessions": 1, "atr_window": 14, "mult": 1.0, "hold": 5, "trend": "none"}),
             "SH13": ({"short_vol": 10, "long_vol": 60, "expansion_ratio": 1.5, "mode": "reversal", "hold": 1},
                      {"short_vol": 10, "long_vol": 60, "expansion_ratio": 1.5, "mode": "reversal", "hold": 5})}
    for fam, (a, b) in pairs.items():
        x, y = sg2.build(sd, fam, a), sg2.build(sd, fam, b)
        assert x.s == y.s and np.array_equal(x.cond, y.cond), fam
        assert not np.array_equal(x.d, y.d), f"{fam}: non-vacuity (execution-only param must change d)"


def test_trial_identity_is_result_independent_and_side_bound():
    cfg = gr.build_configs()[0]
    base = gr.trial_id(gr.trial_identity(cfg, "SPY", IDS))
    polluted = {**cfg, "result": {"net_pnl_usd": 123.4}, "m": {"alpha": 9}, "attempt_id": "x:att0009", "status": "succeeded"}
    assert gr.trial_id(gr.trial_identity(polluted, "SPY", IDS)) == base
    assert set(gr.trial_identity(cfg, "SPY", IDS)) == {"schema_version", "kind", "side", "family", "params", "scope",
                                                         "universe_id", "partitions_id", "protocol_id"}
    assert gr.trial_id(gr.trial_identity({**cfg, "side": "long_short"}, "SPY", IDS)) != base
    assert gr.trial_id(gr.trial_identity(cfg, "QQQ", IDS)) != base
    assert base.startswith("ac02-")
    tags = [gr.tags(c) for c in gr.build_configs()]
    assert sum(t["complement_of_census01"] for t in tags) == 56


def test_retry_is_a_new_attempt_of_the_same_trial(tmp_path):
    cfg = gr.build_configs()[0]
    tid = gr.trial_id(gr.trial_identity(cfg, "SPY", IDS))
    again = gr.trial_id(gr.trial_identity(copy.deepcopy(cfg), "SPY", IDS))
    assert tid == again
    store = ResearchResultStore(tmp_path / "r.sqlite")
    store.register_hypothesis(hypothesis_id="h", experiment_id=pr.EXPERIMENT_ID, hypothesis_text="x")
    store.register_trials_bulk([{"trial_id": tid, "experiment_id": pr.EXPERIMENT_ID, "hypothesis_id": "h", "strategy_id": "s",
                                 "protocol_id": IDS["protocol_id"], "identity": gr.trial_identity(cfg, "SPY", IDS)}])
    (a1, _), = store.begin_attempts_bulk([tid], origin="t")
    store.finalize_attempts_bulk([{"attempt_id": a1, "status": "failed", "failure_reason": "infrastructure_interrupted"}])
    store.begin_attempts_bulk([again], origin="t")
    dig = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    assert list(dig) == [tid] and dig[tid]["attempts"] == 2
    with pytest.raises(pr.DenominatorShrink):
        pr.assert_complete_ledger([tid], [tid, tid])


# ============================================================================================ signals
@pytest.mark.parametrize("cut", [700, 1250, 1700])
def test_every_config_is_causal_prefix_invariant(bars, gappy, cut):
    for frame in (bars, gappy):
        full_sd, short = sg2.build_symbol_data("F", frame), sg2.build_symbol_data("F", frame.iloc[:cut])
        for c in gr.build_configs():
            full, part = sg2.build(full_sd, c["family"], c["params"]), sg2.build(short, c["family"], c["params"])
            if part is None:
                continue
            assert full is not None and full.s == part.s, c["family"]
            assert np.array_equal(full.d[:cut], part.d), (c["family"], c["params"])
            assert np.array_equal(full.cond[:cut], part.cond), (c["family"], c["params"])


def test_prefix_fixtures_are_non_vacuous_for_every_family(bars, gappy):
    """Causality is only proven where a family actually fires: every family must have active configs, and the event
    families that never fire on plain noise (gap, vol-expansion) must fire on the gappy fixture."""
    active = {}
    for frame in (bars, gappy):
        sdx = sg2.build_symbol_data("F", frame)
        for c in gr.build_configs():
            sig = sg2.build(sdx, c["family"], c["params"])
            if sig is not None and (sig.d != 0).any():
                active[c["family"]] = active.get(c["family"], 0) + 1
    assert set(active) == set(gr.EXPECTED_H_COUNTS) | set(gr.EXPECTED_L_COUNTS)
    assert active["SH08"] >= 10 and active["SH13"] >= 12


def test_short_families_never_go_long_and_ls_is_two_sided(sd):
    seen_short = seen_long = 0
    for c in gr.build_configs():
        sig = sg2.build(sd, c["family"], c["params"])
        if sig is None:
            continue
        assert sig.d.dtype == np.int8
        if c["side"] == "short":
            assert sig.d.max() <= 0, c["family"]
            seen_short += int((sig.d < 0).any())
        else:
            seen_long += int((sig.d > 0).any())
            assert (sig.d < 0).any() and (sig.d > 0).any() or c["family"] == "LS04"
    assert seen_short > 300 and seen_long > 20


def test_mirror_semantics_on_hand_series():
    # close rising then falling: SH02 shorts only while close < SMA(3)
    c = np.array([10, 11, 12, 13, 12, 11, 10, 9, 8, 9, 10, 11], float)
    n = len(c)
    df = pd.DataFrame({"symbol": "T", "end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[:n]],
                       "open": c, "high": c + 0.5, "low": c - 0.5, "close": c, "volume": 1e6})
    s = sg2.build_symbol_data("T", df)
    sig = sg2.build(s, "SH02", {"sma": 3})
    sma = pd.Series(c).rolling(3).mean().to_numpy()
    expected = np.where(np.isfinite(sma) & (c < sma), -1, 0)
    assert np.array_equal(sig.d, expected.astype(np.int8)) and sig.s == 2 and (sig.d < 0).any()


def test_state_mirrors_are_exact_complements_of_census01(sd):
    for fam1, fam2, p in (("S01", "SH01", {"lookback": 63, "cadence": "daily"}), ("S02", "SH02", {"sma": 100}),
                          ("S03", "SH03", {"fast": 20, "slow": 100})):
        lg, sh = sg1.PER_SYMBOL[fam1](sd, p), sg2.build(sd, fam2, p)
        assert lg.s == sh.s
        long_on, short_on = lg.d, sh.d < 0
        assert not (long_on & short_on).any()
        defined = np.arange(sd.n) >= lg.s
        assert (long_on | short_on)[defined].mean() > 0.99          # only exact ties are neither


@pytest.mark.parametrize("day", ["2024-01-02", "2025-01-02", "2026-03-02"])
def test_symbol_data_builder_is_fenced_before_any_array_is_built(day, bars):
    bad = pd.concat([bars.iloc[:50], bars.iloc[:1].assign(end_ts=pd.Timestamp(day, tz="UTC") + pd.Timedelta(hours=5))])
    with pytest.raises(pr.pt.PartitionBreach):
        sg2.build_symbol_data("T", bad)


def test_s13_upside_expansion_fade_is_the_mirror_of_the_census01_reversal(sd):
    p = {"short_vol": 10, "long_vol": 60, "expansion_ratio": 1.5, "hold": 1}
    lg = sg1.PER_SYMBOL["S13"](sd, {**p, "mode": "breakout"})          # expansion on an UP day, long
    sh = sg2.build(sd, "SH13", {**p, "mode": "reversal"})              # same expansion on an UP day, faded short
    assert np.array_equal(lg.cond, sh.cond) and lg.s == sh.s
    assert np.array_equal(lg.d, sh.d < 0)


def test_ls_conflicting_states_are_flat_not_an_arbitrary_side():
    n = 400
    c = 10.0 + 0.001 * np.sin(np.arange(n))                    # a flat range: near-high AND near-low both hold
    df = pd.DataFrame({"symbol": "T", "end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[:n]],
                       "open": c, "high": c + 0.01, "low": c - 0.01, "close": c, "volume": 1e6})
    s = sg2.build_symbol_data("T", df)
    p = {"lookback": 126, "distance": 0.10, "cadence": "daily"}
    lg = sg1.PER_SYMBOL["S10"](s, {"high_lookback": 126, "distance": 0.10, "cadence": "daily"})
    sh = sg2.build(s, "SH10", {"low_lookback": 126, "distance": 0.10, "cadence": "daily"})
    both = lg.d & (sh.d < 0)
    assert both.sum() > 100                                    # non-vacuous: the conflict really occurs
    ls = sg2.build(s, "LS04", p)
    assert (ls.d[both] == 0).all() and (ls.d != 0).sum() == 0


# ============================================================================================ simulator
def hand_bars():
    o = [10.0, 10, 10, 10, 10, 10, 10, 10, 10]
    c = [10.0, 10.2, 10.0, 9.8, 9.5, 9.2, 9.6, 9.9, 10.1]
    h = [10.3, 10.4, 10.3, 10.0, 9.9, 9.6, 9.9, 10.2, 10.3]
    l = [9.8, 9.9, 9.7, 9.4, 9.2, 8.9, 9.3, 9.7, 9.9]
    return micros(h, l, c)


def test_same_bar_short_fill_rejected_and_fill_is_first_bar_after_signal():
    hm, lm, cm = hand_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1                                        # desired short after bars 2,3,4; flat decision after bar 5
    so = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0)
    assert list(so.entries) == [3] and list(so.exits) == [6]       # fills on t+1: never on the signal bar
    assert so.net[2] == 0.0 and so.cost[2] == 0.0                  # nothing happens on the decision bar
    qty = 10_000_000_000 // int(cm[2])
    sell3 = int(lm[3]) - int(lm[3]) * 5 // 10_000
    buy6 = int(hm[6]) + int(hm[6]) * 5 // 10_000
    exp_entry_cost = (qty * (int(cm[3]) - sell3) + qty * sell3 * 0.001) / 1e6
    exp_cover_cost = (qty * (buy6 - int(cm[6])) + qty * buy6 * 0.001) / 1e6
    assert so.cost[3] == pytest.approx(exp_entry_cost, abs=1e-9)
    assert so.cost[6] == pytest.approx(exp_cover_cost, abs=1e-9)
    gross = -qty * (int(cm[4]) - int(cm[3]) + int(cm[5]) - int(cm[4]) + int(cm[6]) - int(cm[5])) / 1e6
    assert so.net.sum() == pytest.approx(gross - exp_entry_cost - exp_cover_cost, abs=1e-9)
    assert gross > 0 and len(so.run_pnl) == 1 and so.run_pnl[0] == pytest.approx(so.net.sum(), abs=1e-9)


def test_cover_chronology_future_bars_cannot_change_the_cover():
    hm, lm, cm = hand_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1
    base = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0)
    h2, l2, c2 = hm.copy(), lm.copy(), cm.copy()
    h2[7:], l2[7:], c2[7:] = h2[7:] * 3, l2[7:] // 2, c2[7:] * 2        # perturb only bars after the cover fill
    pert = sim2.simulate_signed(h2, l2, c2, d, 2, borrow_fee_bps_annual=0.0)
    assert np.array_equal(base.net[:7], pert.net[:7]) and list(pert.exits) == [6]


def test_vector_simulator_matches_scalar_reference_with_flips_and_borrow():
    b = synth_bars("R", 5, n=600)
    hm, lm, cm = (np.rint(b[c].to_numpy() * 1e6).astype(np.int64) for c in ("high", "low", "close"))
    rng = np.random.default_rng(3)
    d = np.zeros(600, np.int8)
    i = 5
    while i < 590:
        run = int(rng.integers(1, 12))
        d[i:i + run] = int(rng.choice([-1, 1]))
        i += run + int(rng.integers(0, 3))                 # gap 0 => direct long<->short flip
    so = sim2.simulate_signed(hm, lm, cm, d, 5, borrow_fee_bps_annual=75.0)
    ref = reference(hm, lm, cm, d, fee_bps=75.0)
    assert np.allclose(so.net, ref, atol=1e-6, rtol=0)
    assert (d[1:] * d[:-1] == -1).any(), "fixture must contain direct flips"
    flat = d.copy()
    flat[-8:] = 0
    so2 = sim2.simulate_signed(hm, lm, cm, flat, 5, borrow_fee_bps_annual=75.0)
    assert so2.run_pnl.sum() == pytest.approx(so2.net.sum(), abs=1e-6)   # every run closed: run P&L partitions the total


def test_short_pnl_sign_and_borrow_fee_arithmetic():
    n = 40
    c = np.linspace(10, 20, n)                                 # strictly rising: a short must lose
    h, l, cc = micros(c + 0.05, c - 0.05, c)
    d = np.zeros(n, np.int8)
    d[2:] = -1
    free = sim2.simulate_signed(h, l, cc, d, 2, borrow_fee_bps_annual=0.0)
    fee = sim2.simulate_signed(h, l, cc, d, 2, borrow_fee_bps_annual=252.0 * 100)     # 1% per day for easy arithmetic
    assert free.net.sum() < 0 and free.gross.sum() < 0
    held = np.flatnonzero(free.held_before)
    q = 10_000_000_000 // int(cc[2])                         # sized on the completed signal bar (bar 2)
    expected_fee = sum(q * int(cc[b - 1]) * 0.01 for b in held) / 1e6
    assert (fee.cost.sum() - free.cost.sum()) == pytest.approx(expected_fee, rel=1e-9)


def test_short_without_explicit_borrow_fee_is_refused_never_zero_by_omission():
    hm, lm, cm = hand_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1
    for bad in (None, float("nan"), -1.0):
        with pytest.raises(bw.BorrowRefusal):
            sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=bad)
    long_only = np.zeros(9, np.int8)
    long_only[2:5] = 1
    sim2.simulate_signed(hm, lm, cm, long_only, 2)               # no short leg: no fee needed


def test_fwd_ret_labels_are_never_executable_pnl():
    n = 50
    fwd_ret = np.random.default_rng(1).normal(0, 0.02, n)
    d = np.zeros(n, np.int8)
    d[3:] = -1
    with pytest.raises(TypeError):
        sim2.simulate_signed(fwd_ret, fwd_ret, fwd_ret, d, 3, borrow_fee_bps_annual=0.0)
    as_ints = np.rint(fwd_ret * 1e6).astype(np.int64)
    with pytest.raises(TypeError):
        sim2.simulate_signed(as_ints, as_ints, as_ints, d, 3, borrow_fee_bps_annual=0.0)
    with pytest.raises(bw.BorrowRefusal):
        pr.assert_executable_record({"d": "EVALUABLE", "executable_pnl": False, "evidence_class": bw.EVIDENCE_C})
    with pytest.raises(TypeError):
        sim2.simulate_signed(*hand_bars(), np.zeros(9, np.int64), 2)


def test_benchmark_is_same_direction_hold_not_long_hold(sd):
    n = 300
    c = np.linspace(10, 30, n)
    h, l, cc = micros(c + 0.05, c - 0.05, c)
    always_short = np.full(n, -1, np.int8)
    so = sim2.simulate_signed(h, l, cc, always_short, 0, borrow_fee_bps_annual=0.0)
    hold = sim2.simulate_signed(h, l, cc, sim2.benchmark_hold(n, 0, sim2.benchmark_direction("short", always_short)), 0,
                                borrow_fee_bps_annual=0.0)
    long_hold = sim2.simulate_signed(h, l, cc, sim2.benchmark_hold(n, 0, 1), 0)
    assert so.net.sum() < 0 and so.net.sum() == pytest.approx(hold.net.sum())
    assert so.net.sum() - hold.net.sum() == pytest.approx(0.0)                 # alpha of a constant short vs a short hold
    assert so.net.sum() - long_hold.net.sum() < -1000                          # the sign error would fabricate -2x market
    with pytest.raises(ValueError):
        sim2.benchmark_direction("long_short", always_short)


def test_ssr_flag_hand_cases():
    lm = np.array([100, 100, 89, 95, 96], np.int64) * 1_000_000
    cm = np.array([100, 100, 95, 96, 97], np.int64) * 1_000_000
    assert sim2.ssr_flag(lm, cm, 2) is True          # fill bar itself broke -10% from the prior close
    assert sim2.ssr_flag(lm, cm, 3) is True          # restriction carries to the next session
    assert sim2.ssr_flag(lm, cm, 4) is False and sim2.ssr_flag(lm, cm, 1) is False


# ======================================================================== side-aware benchmark (D2)
def _cell_metrics(net, short_hold_alpha=None):
    m = {"cash_zero": {"net_pnl_usd": net}}
    if short_hold_alpha is not None:
        m["passive_short_hold"] = {"net_alpha_usd": short_hold_alpha}
    return m


R_SIDE, R_NET = sim2.BENCHMARK_RULES


@pytest.mark.parametrize("net,alpha,expect", [(5, 3, True), (5, -3, False), (-1, 9, False), (0, 9, False), (5, 0, False)])
def test_short_only_needs_net_positive_and_alpha_vs_passive_short_hold(net, alpha, expect):
    assert sim2.qualifies("short", _cell_metrics(net, alpha), R_SIDE) is expect


@pytest.mark.parametrize("net,expect", [(5, True), (0.01, True), (0, False), (-5, False)])
def test_long_short_qualifies_on_net_vs_cash_and_never_consults_a_hold(net, expect):
    only_cash = _cell_metrics(net)                                   # no passive-hold record at all: must not be required
    assert sim2.qualifies("long_short", only_cash, R_SIDE) is expect
    losing_both_holds = {**only_cash, "passive_long_hold": {"net_alpha_usd": -1e9}, "passive_short_hold": {"net_alpha_usd": -1e9}}
    assert sim2.qualifies("long_short", losing_both_holds, R_SIDE) is expect     # diagnostics cannot veto a switching strategy


def test_benchmark_rules_fail_closed():
    with pytest.raises(KeyError):
        sim2.qualifies("short", _cell_metrics(5), R_SIDE)              # short-only record missing its passive short hold
    with pytest.raises(ValueError):
        sim2.qualifies("long", _cell_metrics(5), R_SIDE)
    with pytest.raises(ValueError):
        sim2.qualifies("short", _cell_metrics(5, 1), "NET_POSITIVE_AND_SAME_DIRECTION_HOLD_ALPHA_POSITIVE")
    assert sim2.qualifies("short", _cell_metrics(5), R_NET) is True    # the net-only rule needs no hold
    with pytest.raises(ValueError):
        sim2.benchmark_direction("long_short", np.zeros(3, np.int8))   # no single direction for a switching strategy


def test_evaluated_cells_record_side_aware_benchmarks_with_matched_cost_and_borrow(sd):
    short_cfg = next(c for c in gr.build_configs() if c["family"] == "SH02")
    ls_cfg = next(c for c in gr.build_configs() if c["family"] == "LS02")
    a = sim2.evaluate_short_cell(sd, short_cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=100)
    b = sim2.evaluate_short_cell(sd, ls_cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=100)
    assert set(a["m"]) == {"cash_zero", "passive_short_hold"} and a["benchmark_roles"]["passive_short_hold"] == "QUALIFICATION_ALPHA"
    assert set(b["m"]) == {"cash_zero", "passive_long_hold", "passive_short_hold"}
    assert b["benchmark_roles"] == {"cash_zero": "QUALIFICATION_NET", "passive_long_hold": "DIAGNOSTIC_ONLY",
                                    "passive_short_hold": "DIAGNOSTIC_ONLY"}
    # the passive short hold carries the same borrow fee: dropping it must change the benchmark cost
    free = sim2.evaluate_short_cell(sd, short_cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=0)
    assert a["m"]["passive_short_hold"]["benchmark_cost_usd"] > free["m"]["passive_short_hold"]["benchmark_cost_usd"]
    # a long hold never accrues borrow
    assert b["m"]["passive_long_hold"]["benchmark_cost_usd"] == sim2.evaluate_short_cell(
        sd, ls_cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=0)["m"]["passive_long_hold"]["benchmark_cost_usd"]
    assert sim2.qualifies("short", a["m"], R_SIDE) in (True, False) and sim2.qualifies("long_short", b["m"], R_SIDE) in (True, False)


# =================================================================== short conditional factor direction (D4)
CTX = {"universe_identity": {"universe_id": "u", "symbols_sha256": "s"}, "data_provenance_identity": {"bars": "b"}}


def test_short_factors_are_lower_is_better_and_the_population_is_exact():
    conds = gr.build_conditions(gr.build_configs())
    specs = [(c, h, sp) for c, h, sp in fx.iter_factor_specs(conds, CTX)]
    assert len(specs) == 1075 and {sp.direction for _c, _h, sp in specs} == {"lower_is_better"}
    ids = fx.expected_factor_ids(conds, CTX)
    assert len(set(ids)) == 1075 and fx.FACTOR_FAMILY.startswith("alpha_census02")
    assert pr.build_structural_protocol()["conditional_factor_semantics"]["direction"] == "lower_is_better"
    assert all(sp.params["side"] == "short" for _c, _h, sp in specs)
    cut = gr.build_conditions(gr.build_configs(complements_excluded=True), complements_excluded=True)
    assert len(fx.expected_factor_ids(cut, CTX)) == 935


def test_direction_is_identity_bound_and_a_flip_is_refused():
    conds = gr.build_conditions(gr.build_configs())
    cond = conds[0]
    good = fx.factor_spec(cond, 5, CTX)
    flipped = fx.factor_spec(cond, 5, CTX, direction="higher_is_better")
    assert good.compute_factor_id() != flipped.compute_factor_id()                 # direction is in the factor identity
    assert good.identity_payload()["direction"] == "lower_is_better"
    assert fx.require_registered_short_factor(good, conds, CTX) == good.compute_factor_id()
    with pytest.raises(fx.FactorAuthorityRefusal):
        fx.require_registered_short_factor(flipped, conds, CTX)
    import dataclasses
    with pytest.raises(fx.FactorAuthorityRefusal):                                  # same direction label, different identity field
        fx.require_registered_short_factor(dataclasses.replace(good, horizon_periods=3), conds, CTX)
    with pytest.raises(fx.FactorAuthorityRefusal):
        fx.direction_adjusted_effect(0.01, "higher_is_better")


def test_direction_adjusted_effect_is_minus_the_raw_effect_and_matches_census01_diagnostics():
    assert fx.direction_adjusted_effect(-0.02) == pytest.approx(0.02) and fx.direction_adjusted_effect(0.03) == pytest.approx(-0.03)
    import conditional as cd1
    n = 6
    frame = pd.DataFrame({"symbol": ["A"] * n, "factor_value": [1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
                          "label_fwd_ret": [-0.02, -0.04, 0.0, 0.01, 0.01, 0.01]})
    aux = pd.DataFrame({"year": [2016] * n, "regime": [1] * n})
    d = cd1.event_diagnostics(frame, aux, "lower_is_better")
    assert d["effect"] == pytest.approx(-0.02) and d["direction_adjusted_effect"] == pytest.approx(fx.direction_adjusted_effect(d["effect"]))
    assert d["direction_adjusted_effect"] > 0                                       # falling after the event favors the short


def test_condition_lookback_matches_the_first_defined_bar_of_the_builder(gappy):
    sdg = sg2.build_symbol_data("G", gappy)
    checked = 0
    for cond in gr.build_conditions(gr.build_configs()):
        cfg = next(c for c in gr.build_configs() if c["side"] == "short" and c["family"] == cond["family"]
                   and gr.condition_params(c["family"], c["params"]) == cond["params"])
        sig = sg2.build(sdg, cfg["family"], cfg["params"])
        assert sig is not None
        base = fx.condition_lookback(cond["family"], cond["params"])
        exec_pad = max([cfg["params"].get("exit", 0)] if cond["family"] in ("SH04", "SH09") else [0])
        assert sig.s == max(base, exec_pad), (cond["family"], cond["params"], sig.s, base)
        checked += 1
    assert checked == 215


# ==================================================================== fail-closed simulator inputs (D8)
def _ok_bars():
    return hand_bars()


def test_zero_or_negative_low_is_refused_even_when_close_is_positive():
    hm, lm, cm = _ok_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1
    for bad_low in (0, -1):
        lm2 = lm.copy()
        lm2[4] = bad_low                                              # high >= close >= low holds; low > 0 does not
        assert (hm >= cm).all() and (cm >= lm2).all() and (cm > 0).all()
        with pytest.raises(TypeError):
            sim2.simulate_signed(hm, lm2, cm, d, 2, borrow_fee_bps_annual=0.0)


@pytest.mark.parametrize("kw", [
    {"commission_bps": -1.0}, {"commission_bps": float("nan")}, {"commission_bps": float("inf")}, {"commission_bps": True},
    {"commission_bps": "10"}, {"slippage_bps": -1}, {"slippage_bps": 2.5}, {"slippage_bps": float("nan")},
    {"slippage_bps": True}, {"slippage_bps": "5"}, {"slippage_bps": np.float64(5.0)}])
def test_malformed_cost_overrides_are_refused_not_coerced(kw):
    hm, lm, cm = _ok_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1
    with pytest.raises(ValueError):
        sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0, **kw)


def test_valid_cost_overrides_are_accepted_and_malformed_borrow_fee_is_refused_even_without_a_short():
    hm, lm, cm = _ok_bars()
    d = np.zeros(9, np.int8)
    d[2:5] = -1
    base = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0)
    hi = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0, commission_bps=20.0, slippage_bps=np.int64(10))
    assert hi.cost.sum() > base.cost.sum() > 0
    long_only = np.zeros(9, np.int8)
    long_only[2:5] = 1
    for bad in (-1.0, float("nan"), True):
        with pytest.raises(bw.BorrowRefusal):
            sim2.simulate_signed(hm, lm, cm, long_only, 2, borrow_fee_bps_annual=bad)


# ============================================================================================ borrow capability
def test_borrow_classification_and_individual_equity_never_executable(sd):
    assert bw.classify_evidence("SPY", None) == bw.EVIDENCE_A                  # no frozen assumption: nothing is executable
    assert bw.classify_evidence("SPY", ASSUME) == bw.EVIDENCE_C
    assert bw.classify_evidence("TSLA", ASSUME) == bw.EVIDENCE_A               # outside the frozen ETF scope
    cfg = next(c for c in gr.build_configs() if c["family"] == "SH02")
    rec = sim2.evaluate_short_cell(sd, cfg, bw.classify_evidence("TSLA", ASSUME), borrow_fee_bps_annual=50)
    assert rec["m"] is None and rec["executable_pnl"] is False and rec["d"].startswith("HYPOTHESIS_ONLY")
    with pytest.raises(bw.BorrowRefusal):
        pr.assert_executable_record(rec)
    with pytest.raises(bw.BorrowRefusal):
        bw.require_executable(bw.EVIDENCE_A)
    with pytest.raises(bw.BorrowRefusal):
        bw.require_executable(bw.EVIDENCE_B)                                   # class B needs point-in-time data
    ok = sim2.evaluate_short_cell(sd, cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=50)
    assert ok["executable_pnl"] is True and set(ok["m"]) == {"cash_zero", "passive_short_hold"}
    assert pr.assert_executable_record(ok) is ok






def test_proposal_artifact_cannot_satisfy_the_freeze_and_holds_no_result():
    doc = json.loads(pr.PROPOSAL_FILE.read_text(encoding="utf-8"))
    assert doc["status"] == pr.STATUS_PROPOSED and doc["real_census02_attempts_executed"] == 0
    assert doc["confirmation_rows_consumed"] == 0 and doc["final_holdout_rows_consumed"] == 0
    import c2_proposal
    assert doc == json.loads(json.dumps(c2_proposal.build_proposal())), "committed proposal drifted from the grammar authority"
    assert sorted(doc["operator_decisions_required"]) == sorted(pr.DECISIONS)
    assert doc["candidate_grammar"]["tier_H_configs"] == 430 and doc["candidate_grammar"]["tier_L_configs"] == 40
    assert doc["candidate_grammar"]["short_conditions"] == 215 and doc["candidate_grammar"]["conditional_factors"] == 1075
    cha = doc["candidate_grammar"]["complement_handling_arithmetic"]
    assert cha["REGISTER_ALL_TAG_COMPLEMENTS"] == {"configs": 470, "conditions": 215, "conditional_factors": 1075}
    assert {k: cha["EXCLUDE_COMPLEMENTS_BEFORE_FREEZE"][k] for k in ("configs", "conditions", "conditional_factors")} == {
        "configs": 414, "conditions": 187, "conditional_factors": 935}
    assert doc["conditional_factor_semantics"]["direction"] == "lower_is_better"
    assert doc["operator_decisions_required"]["conditional_scope"] == ["ALL_SEED_SYMBOLS"]
    assert doc["benchmark_contract"]["roles"]["long_short"]["passive_long_hold"] == "DIAGNOSTIC_ONLY"
    assert set(doc["behavior_source_manifest_scope"]["sources"]) == set(pr.BEHAVIOR_SOURCES)
    rec = doc["independent_review_recommendations"]
    assert rec["status"].startswith("RECOMMENDED_AWAITING") and rec["status"].endswith("NOT_FROZEN")
    assert doc["status"] == pr.STATUS_PROPOSED
    assert doc["structural_protocol_id"] == pr.sha256_canonical(pr.build_structural_protocol())[:32]
    blob = json.dumps(doc)
    for forbidden in ("net_pnl", "alpha_usd", "sharpe", "p_value", "survivors"):
        assert forbidden not in blob
    with pytest.raises(pr.FreezeRefusal):
        pr.require_freeze(pr.REPO, pr.PROPOSAL_FILE)


# ===================================================================================== stale census-01 paths
def test_census01_executable_paths_refuse_or_never_see_census02_configs(sd):
    c2 = gr.build_configs()
    with pytest.raises((ss1.GrammarRefusal, KeyError)):
        ss1.assert_grammar_authority([{"family": c["family"], "scope": "symbol"} for c in c2])
    assert not set(gr.H_FAMILIES) & set(sg1.PER_SYMBOL)
    with pytest.raises(KeyError):
        sg2.build(sd, "S02", {"sma": 50})                                     # Census-01 family id is not a Census-02 family


def test_census01_simulator_is_bool_only_and_silently_flips_shorts_to_longs():
    """Hazard proof (Census-01 contract unchanged): its simulate() casts a signed series to bool, so a short becomes a
    long. Census-02 therefore never routes through it."""
    hm, lm, cm = hand_bars()
    sgnd = np.zeros(9, np.int8)
    sgnd[2:5] = -1
    legacy = sm1.simulate(hm, lm, cm, sgnd, 2)
    as_long = sm1.simulate(hm, lm, cm, (sgnd != 0), 2)
    assert np.array_equal(legacy.net, as_long.net)
    real = sim2.simulate_signed(hm, lm, cm, sgnd, 2, borrow_fee_bps_annual=0.0)
    assert real.gross.sum() == pytest.approx(-legacy.gross.sum()) and real.gross.sum() > 0 > legacy.gross.sum()
