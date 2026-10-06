"""Corrected Alpha Edge Census invariants: authority, partitions, eligibility, causality, execution, store."""

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
import census as cs  # noqa: E402
import conditional as cd  # noqa: E402
import data as cdata  # noqa: E402
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


def _load(name):
    return json.loads((EXP / name).read_text(encoding="utf-8"))


# --------------------------------------------------------------------------------------- authority / identity

def test_frozen_authority_manifests_match_regenerated_authority():
    seed, grammar = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json"), _load("ALPHA_CENSUS_GRAMMAR_V2.json")
    assert _load("ALPHA_CENSUS_PARTITIONS_V2.json") == ss.build_partitions()
    assert _load("ALPHA_CENSUS_PROTOCOL_V2.json") == ss.build_protocol()
    assert grammar == ss.build_grammar()
    assert seed["symbol_count"] == len(seed["symbols"]) == 88 and seed["symbols"] == sorted(set(seed["symbols"]))
    assert "AAPL" in seed["symbols"]
    assert seed["universe_source_kind"] == "current_enabled_equity_registry_snapshot_v1"
    assert seed["point_in_time_membership"] is False
    assert seed["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"
    rejected = json.loads((EXP / "rejected_run_20261005" / "ALPHA_CENSUS_UNIVERSE_V1.json").read_text(encoding="utf-8"))
    assert seed["symbols"] == rejected["symbols"]


def test_ir2_families_are_exactly_s01_to_s14():
    assert tuple(sorted({c["family"] for c in ss.build_configs()})) == tuple(f"S{i:02d}" for i in range(1, 15))
    assert tuple(ss.FAMILIES) == ss.EXPECTED_FAMILY_IDS
    assert all(c["scope"] == "symbol" for c in ss.build_configs())


def test_ir3_exact_corrected_strategy_edge_configuration_count_is_434():
    cfgs = ss.build_configs()
    assert len(cfgs) == 434 == ss.EXPECTED_CONFIG_COUNT
    assert len({c["config_id"] for c in cfgs}) == 434
    assert sum(ss.EXPECTED_FAMILY_COUNTS.values()) == 434


@pytest.mark.parametrize("family,count", [("S01", 8), ("S02", 6), ("S03", 14), ("S04", 12), ("S05", 72), ("S06", 24),
                                          ("S07", 108), ("S08", 36), ("S09", 64), ("S10", 12), ("S11", 18),
                                          ("S12", 32), ("S13", 24), ("S14", 4)])
def test_per_family_config_counts(family, count):
    assert sum(1 for c in ss.build_configs() if c["family"] == family) == count


def _axis(family, key):
    return sorted({c["params"][key] for c in ss.build_configs() if c["family"] == family})


def test_ir4_axes_equal_the_frozen_values():
    assert _axis("S01", "lookback") == [21, 63, 126, 252] and _axis("S01", "cadence") == ["daily", "month_end"]
    assert _axis("S02", "sma") == [20, 50, 100, 150, 200, 250]
    assert _axis("S03", "fast") == [10, 20, 50] and _axis("S03", "slow") == [50, 100, 150, 200, 250]
    assert all(c["params"]["fast"] < c["params"]["slow"] for c in ss.build_configs() if c["family"] == "S03")
    assert _axis("S04", "entry") == [20, 50, 100, 150, 200] and _axis("S04", "exit") == [10, 20, 50]
    assert all(c["params"]["exit"] < c["params"]["entry"] for c in ss.build_configs() if c["family"] == "S04")
    assert _axis("S05", "period") == [2, 3, 5, 7, 10, 14] and _axis("S05", "entry_below") == [10, 20, 30]
    assert _axis("S05", "exit_above") == [50, 70] and _axis("S05", "trend") == ["none", "sma200"]
    assert _axis("S06", "lookback") == [10, 20, 40, 60] and _axis("S06", "entry_z") == [-2.5, -2.0, -1.5]
    assert _axis("S06", "exit_z") == [0.0] and _axis("S06", "trend") == ["none", "sma200"]
    assert _axis("S07", "decline_sessions") == [1, 3, 5] and _axis("S07", "atr_window") == [14, 20]
    assert _axis("S07", "mult") == [1.0, 1.5, 2.0] and _axis("S07", "hold") == [1, 3, 5]
    assert _axis("S07", "trend") == ["none", "sma200"]
    assert set(next(c for c in ss.build_configs() if c["family"] == "S07")["params"]) == {
        "decline_sessions", "atr_window", "mult", "hold", "trend"}
    assert _axis("S08", "atr_window") == [14, 20] and _axis("S08", "mult") == [1.0, 1.5, 2.0]
    assert _axis("S08", "hold") == [1, 3, 5] and _axis("S08", "trend") == ["none", "sma200"]
    assert set(next(c for c in ss.build_configs() if c["family"] == "S08")["params"]) == {
        "atr_window", "mult", "hold", "trend"}
    assert _axis("S09", "short") == [5, 10] and _axis("S09", "long") == [40, 60] and _axis("S09", "ratio") == [0.25, 0.40]
    assert _axis("S09", "breakout") == [20, 50] and _axis("S09", "exit") == [10, 20]
    assert _axis("S10", "high_lookback") == [126, 252] and _axis("S10", "distance") == [0.03, 0.05, 0.10]
    assert _axis("S10", "cadence") == ["daily", "month_end"]
    assert _axis("S11", "down_sessions") == [2, 3, 4] and _axis("S11", "hold") == [1, 3, 5]
    assert _axis("S12", "volume_lookback") == [20, 60] and _axis("S12", "volume_z") == [1.5, 2.0]
    assert _axis("S12", "price_impulse") == [1, 5] and _axis("S12", "mode") == ["continuation", "reversal"]
    assert _axis("S12", "hold") == [1, 3]
    assert _axis("S13", "short_vol") == [10, 20] and _axis("S13", "long_vol") == [60]
    assert _axis("S13", "expansion_ratio") == [1.5, 2.0] and _axis("S13", "mode") == ["breakout", "reversal"]
    assert _axis("S13", "hold") == [1, 3, 5]
    s14 = [c["params"] for c in ss.build_configs() if c["family"] == "S14"]
    assert s14 == [{"kind": "tom", "last": 1, "first": 3}, {"kind": "tom", "last": 2, "first": 3},
                   {"kind": "tom", "last": 1, "first": 5}, {"kind": "season", "value": "nov_apr"}]


def _grammar_mutants():
    cfgs = ss.build_configs()
    extra = {"config_id": "z", "family": "S15", "scope": "symbol", "params": {"x": 1}}
    yield "added S15", cfgs + [extra]
    yield "dropped one config", cfgs[:-1]
    yield "extra S07 coordinate", cfgs + [{**cfgs[0], "family": "S07", "config_id": "q", "params": {"hold": 99}}]
    yield "universe scope", [{**cfgs[0], "scope": "universe"}] + cfgs[1:]
    yield "family missing", [c for c in cfgs if c["family"] != "S14"]


@pytest.mark.parametrize("name,mutant", list(_grammar_mutants()), ids=lambda v: v if isinstance(v, str) else "")
def test_ir5_grammar_authority_refuses_any_drift_before_attempt_one(name, mutant):
    with pytest.raises(ss.GrammarRefusal):
        ss.assert_grammar_authority(mutant)


def test_ir5_s15_to_s20_are_not_registered_families():
    assert not {f"S{i}" for i in range(15, 21)} & set(ss.FAMILIES)
    assert ss.build_grammar()["rejected_grammar"]["label"] == "REJECTED_UNAUTHORIZED_SEARCH_GRAMMAR"


def test_ir6_conditional_horizons_are_exactly_1_3_5_10_20():
    assert ss.CONDITIONAL_HORIZONS == (1, 3, 5, 10, 20) and 2 not in ss.CONDITIONAL_HORIZONS
    assert ss.build_grammar()["conditional_horizons"] == [1, 3, 5, 10, 20]
    assert ss.build_protocol()["conditional_edge"]["horizons"] == [1, 3, 5, 10, 20]
    assert ss.build_grammar()["conditional_factor_count"] == 434 * 5


def test_trial_count_is_derived_from_the_eligible_universe_never_hard_coded():
    g, part, prot = ss.build_grammar(), ss.build_partitions(), ss.build_protocol()
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    for k in (88, 87, 3):
        disp = {s: {"disposition": cdata.ELIGIBLE if i < k else cdata.EXCLUDED_INSUFFICIENT_HISTORY}
                for i, s in enumerate(seed["symbols"])}
        space = ss.build_search_space(g, ss.build_universe(seed, disp), part, prot)
        assert space["eligible_symbol_count"] == k and space["strategy_trial_count"] == 434 * k
        assert space["config_count"] == 434 and space["seed_symbol_count"] == 88
    assert 434 * 88 == 38192
    with pytest.raises(ss.GrammarRefusal):
        ss.build_search_space(g, ss.build_universe(seed, {s: {"disposition": cdata.EXCLUDED_DATA_UNAVAILABLE}
                                                         for s in seed["symbols"]}), part, prot)


def test_identity_is_param_order_invariant_and_param_sensitive():
    ids = {"universe_id": "u", "partitions_id": "p", "protocol_id": "q"}
    c = {"config_id": "x", "family": "S01", "scope": "symbol", "params": {"a": 1, "b": 2}}
    c_perm = {**c, "params": {"b": 2, "a": 1}}
    c_other = {**c, "params": {"a": 1, "b": 3}}
    t = lambda cfg: ss.trial_id(ss.trial_identity(cfg, "SPY", ids))  # noqa: E731
    assert t(c) == t(c_perm) and t(c) != t(c_other) and t(c).startswith("ace2-")
    assert ss.config_id("S01", {"a": 1, "b": 2}) == ss.config_id("S01", {"b": 2, "a": 1})
    assert ss.config_id("S01", {"a": 1, "b": 2}) != ss.config_id("S01", {"a": 1, "b": 3})
    assert t(c) != ss.trial_id(ss.trial_identity(c, "QQQ", ids))
    assert ss.EXPERIMENT_ID != ss.REJECTED_EXPERIMENT_ID


def test_protocol_declares_corrected_economics_floors_and_deferred_judge():
    prot = ss.build_protocol()
    assert prot["execution_contract"]["execution_pricing"]["slippage_bps"] == 5
    q = prot["strategy_edge_qualification"]
    assert q["min_closed_round_trips"] == 5 and q["min_evaluated_bars"] == 252 and q["judge_status"] == "DEFERRED_FULL_POPULATION"
    c = prot["conditional_edge"]
    assert c["min_events"] == 30 and c["fdr"]["alpha"] == 0.10 and c["null_protocol"] == {
        "n_permutations": 200, "base_seed": 0, "source": c["null_protocol"]["source"]}
    assert prot["eligibility_rule"]["min_valid_completed_1day_observations_strictly_before_2024_01_01"] == 252
    assert prot["eligibility_rule"]["data_present_alone_is_not_a_disposition"] is True
    assert prot["sizing"]["production_default"] is False and prot["chunking"]["cells_per_chunk"] == 500
    assert prot["indicator_conventions"]["zscore"].endswith("ddof=1)")
    assert "feature_version" not in prot and "ml_folds" not in prot


# ----------------------------------------------------------------------------------- partitions / data / SIP

def test_calendar_pin_and_session_grid_end_at_the_last_discovery_session():
    assert cal.CONTENT_SHA256 == cal.EXPECTED_CONTENT_SHA256
    assert SESSIONS[0] >= dt.date(2016, 1, 1) and SESSIONS[-1] == dt.date(2023, 12, 29)


def test_ir23_partition_labels_never_call_2024_an_unread_reserve():
    blob = json.dumps(pt.PARTITIONS)
    assert "RESERVED_UNREAD" not in blob and "UNREAD" not in blob.replace("NEVER_AN_UNREAD_RESERVE", "")
    p = pt.PARTITIONS
    assert p["discovery"]["end_exclusive"] == "2024-01-01"
    assert p["contaminated_by_rejected_run"] == {
        "start_inclusive": "2024-01-01", "end_exclusive": "2025-01-01", "status": "CONTAMINATED_BY_REJECTED_RUN",
        "reason": "read by ALPHA_EDGE_CENSUS_01_REJECTED_EXECUTION_20261005", "role": "NEVER_AN_UNREAD_RESERVE"}
    assert p["remaining_confirmation_reserve"]["start_inclusive"] == "2025-01-01"
    assert p["remaining_confirmation_reserve"]["end_exclusive"] == "2026-03-01"
    assert p["final_holdout"] == {"start_inclusive": "2026-03-01", "status": "RESERVED_UNCONSUMED"}


@pytest.mark.parametrize("ts,label", [("2023-12-29T05:00:00+00:00", pt.DISCOVERY), ("2024-01-01T00:00:00+00:00", pt.CONTAMINATED),
                                      ("2024-12-31T05:00:00+00:00", pt.CONTAMINATED), ("2025-01-01T00:00:00+00:00", pt.RESERVE),
                                      ("2026-02-27T05:00:00+00:00", pt.RESERVE), ("2026-03-01T00:00:00+00:00", pt.HOLDOUT)])
def test_classify_timestamp(ts, label):
    assert pt.classify_timestamp(ts) == label


def test_partition_label_literals_are_frozen_not_self_referential():
    assert (pt.DISCOVERY, pt.CONTAMINATED, pt.RESERVE, pt.HOLDOUT) == (
        "DISCOVERY", "CONTAMINATED_BY_REJECTED_RUN", "REMAINING_CONFIRMATION_RESERVE", "FINAL_HOLDOUT")
    assert pt.classify_timestamp("2024-06-03T05:00:00+00:00") == "CONTAMINATED_BY_REJECTED_RUN"


@pytest.mark.parametrize("ts", ["2024-01-02", "2024-06-28", "2025-01-02", "2025-06-30", "2026-03-02", "2026-09-01"])
def test_ir1_partition_fence_refuses_2024_2025_and_holdout_rows(ts):
    pt.require_discovery_only(pd.Series(["2023-12-29T05:00:00+00:00"]), what="ok")
    with pytest.raises(pt.PartitionBreach):
        pt.require_discovery_only(pd.Series(["2023-12-28T05:00:00+00:00", ts + "T05:00:00+00:00"]), what="bad")


def test_partition_fence_refuses_empty_input():
    with pytest.raises(pt.PartitionBreach):
        pt.require_discovery_only(pd.Series([], dtype=object), what="empty")


@pytest.mark.parametrize("bad_date", ["2024-01-02", "2025-01-02", "2026-03-02", "2020-07-04"])
def test_symbol_data_refuses_non_session_and_post_discovery_dates(bars, bad_date):
    b = bars["X00"].copy()
    b.loc[5, "end_ts"] = pd.Timestamp(bad_date, tz="UTC") + pd.Timedelta(hours=5)
    with pytest.raises(RuntimeError, match="canonical"):
        sg.SymbolData("X00", b)


def test_request_contract_ends_exactly_at_the_discovery_fence():
    assert cdata.REQUEST_CONTRACT["end_utc_exclusive"] == "2024-01-01T00:00:00+00:00"
    assert cdata.REQUEST_CONTRACT["start_utc"] == "2016-01-01T00:00:00+00:00"


def test_sip_all_contract_refuses_other_feed_or_adjustment():
    good = {"source_attestation": {"feed": "sip", "adjustment_mode": "all", "source_provider_id": "alpaca",
                                   "requested_start_utc": "2016-01-01T00:00:00+00:00",
                                   "requested_end_utc": "2024-01-01T00:00:00+00:00"}}
    cdata.require_sip_all_contract(good)
    for k, v in (("feed", "iex"), ("adjustment_mode", "raw"), ("source_provider_id", "other"),
                 ("requested_end_utc", "2025-01-01T00:00:00+00:00"), ("requested_end_utc", "2026-03-02T00:00:00+00:00")):
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


# ------------------------------------------------------------------------------------------- eligibility

def _elig(monkeypatch, bars_df, status=None):
    monkeypatch.setattr(cdata, "load_symbol_bars", lambda d: (bars_df, {"artifact_sha256": "h"}))
    return cdata.classify_eligibility("X00", status or {"disposition": "DATA_PRESENT"}, Path("unused"))


@pytest.mark.parametrize("n,expected", [(252, cdata.ELIGIBLE), (251, cdata.EXCLUDED_INSUFFICIENT_HISTORY),
                                        (1000, cdata.ELIGIBLE), (10, cdata.EXCLUDED_INSUFFICIENT_HISTORY)])
def test_eligibility_requires_252_valid_completed_observations(monkeypatch, bars, n, expected):
    assert _elig(monkeypatch, bars["X00"].iloc[:n].reset_index(drop=True))["disposition"] == expected


def test_ir21_data_present_alone_cannot_satisfy_eligibility(monkeypatch, tmp_path):
    rec = cdata.classify_eligibility("X00", {"disposition": "DATA_PRESENT"}, tmp_path / "missing")
    assert rec["disposition"] == cdata.EXCLUDED_PROVENANCE_REJECTED and rec["disposition"] != "DATA_PRESENT"
    assert "DATA_PRESENT" not in {cdata.ELIGIBLE, *cdata.EXCLUDED_DISPOSITIONS}


def test_eligibility_types_every_non_present_acquisition_status(tmp_path):
    f = lambda st: cdata.classify_eligibility("X", {"disposition": st}, tmp_path)["disposition"]  # noqa: E731
    assert f("NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION") == cdata.EXCLUDED_UNSUPPORTED_CORPORATE_ACTION
    assert f("DATA_UNAVAILABLE_PROVIDER_ERROR") == cdata.EXCLUDED_DATA_UNAVAILABLE
    assert f("DATA_UNAVAILABLE_UNEXPECTED_ERROR") == cdata.EXCLUDED_DATA_UNAVAILABLE


def test_eligibility_excludes_non_canonical_sessions_as_invalid_bar_data(monkeypatch, bars):
    b = bars["X00"].copy()
    b.loc[5, "end_ts"] = pd.Timestamp("2020-07-04", tz="UTC") + pd.Timedelta(hours=5)
    assert _elig(monkeypatch, b)["disposition"] == cdata.EXCLUDED_INVALID_BAR_DATA


def test_ir22_every_seed_symbol_has_exactly_one_typed_disposition():
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    disp = {s: {"disposition": cdata.ELIGIBLE} for s in seed["symbols"]}
    disp[seed["symbols"][3]] = {"disposition": cdata.EXCLUDED_INSUFFICIENT_HISTORY}
    uni = ss.build_universe(seed, disp)
    assert uni["symbol_count"] == 87 and sorted(uni["excluded"]) == [seed["symbols"][3]]
    assert sorted(uni["dispositions"]) == seed["symbols"]
    for bad in ({**disp, seed["symbols"][0]: {"disposition": "DATA_PRESENT"}},
                {**disp, seed["symbols"][0]: {"disposition": "EXCLUDED_BECAUSE_IT_LOST_MONEY"}},
                {s: d for s, d in disp.items() if s != seed["symbols"][0]},
                {**disp, "ZZZZ": {"disposition": cdata.ELIGIBLE}}):
        with pytest.raises(RuntimeError):
            ss.build_universe(seed, bad)


def test_seed_builder_fails_closed_when_snapshot_claims_point_in_time(monkeypatch):
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
        ss.build_seed_universe()


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


# ------------------------------------------------------------------------- causal signals (synthetic)

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
    fams = set()
    for c in first_configs(3):
        for sym in ("SPY", "X03", "X04"):
            assert prefix_invariant(U, U2, sym, c["family"], c["params"], boundary), (c["family"], c["params"], sym)
            fams.add(c["family"])
    assert fams == set(ss.EXPECTED_FAMILY_IDS)
    for fam in ss.EXPECTED_FAMILY_IDS:
        assert any(U.build("X03", c["family"], c["params"]) is not None for c in ss.build_configs() if c["family"] == fam)


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


def test_month_end_cadence_only_changes_decision_after_a_month_end_anchor(U):
    for fam, p in (("S01", {"lookback": 63, "cadence": "month_end"}),
                   ("S10", {"high_lookback": 126, "distance": 0.10, "cadence": "month_end"})):
        sd = U.sd["X03"]
        sig = U.build("X03", fam, p)
        change = np.flatnonzero(sig.d[1:] != sig.d[:-1]) + 1
        assert len(change) > 0 and sd.month_end[change].all(), fam
        assert np.all(~sig.cond | sd.month_end)


def _flat_symbol(n=60):
    ts = [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in SESSIONS[:n]]
    return pd.DataFrame({"symbol": "T", "end_ts": ts, "open": 100.0, "high": 101.0, "low": 99.0, "close": 100.0, "volume": 1e6})


def _events(df, family, params):
    sig = sg.PER_SYMBOL[family](sg.SymbolData("T", df), params)
    return sig, np.flatnonzero(sig.cond), np.flatnonzero(sig.d)


def test_s08_gap_down_uses_prior_bar_atr_and_holds_exactly_n_bars():
    df = _flat_symbol()
    df.loc[30, ["open", "high", "low", "close"]] = [96.0, 101.0, 95.0, 100.0]
    _sig, ev, held = _events(df, "S08", {"atr_window": 14, "mult": 1.0, "hold": 3, "trend": "none"})
    assert list(ev) == [30] and list(held) == [30, 31, 32]
    df.loc[30, "open"] = 98.5  # gap of 1.5 is inside the 2.0 prior-bar ATR threshold
    _sig, ev, _ = _events(df, "S08", {"atr_window": 14, "mult": 1.0, "hold": 3, "trend": "none"})
    assert len(ev) == 0


def test_s07_decline_uses_prior_bar_atr_so_the_event_bar_range_cannot_mask_itself():
    df = _flat_symbol()
    df.loc[30, ["open", "high", "low", "close"]] = [100.0, 100.0, 60.0, 95.0]  # huge same-bar range, 5pt decline
    _sig, ev, held = _events(df, "S07", {"decline_sessions": 3, "atr_window": 14, "mult": 1.0, "hold": 1, "trend": "none"})
    assert list(ev) == [30] and list(held) == [30]  # prior-bar ATR is 2.0; a same-bar ATR (~4.8) would reject it


def test_s05_matches_hand_computed_cutler_rsi(bars, U):
    c = bars["X01"]["close"].to_numpy()
    n, entry = 3, 30
    d = np.diff(c, prepend=np.nan)
    up, down = np.where(d > 0, d, 0.0), np.where(d < 0, -d, 0.0)
    rsi = np.full(len(c), np.nan)
    for t in range(n, len(c)):
        u, w = up[t - n + 1:t + 1].mean(), down[t - n + 1:t + 1].mean()
        rsi[t] = 100.0 if (w == 0 and u > 0) else (np.nan if w == 0 else 100.0 - 100.0 / (1.0 + u / w))
    sig = U.build("X01", "S05", {"period": n, "entry_below": entry, "exit_above": 50, "trend": "none"})
    s = int(np.flatnonzero(np.isfinite(rsi))[0])
    expect = (np.nan_to_num(rsi, nan=1e9) < entry)
    expect[:s] = False
    assert sig.s == s and np.array_equal(sig.cond, expect) and expect.any()


def test_s06_zscore_uses_sample_std_ddof1(bars, U):
    c, n = bars["X02"]["close"].to_numpy(), 20
    z = np.full(len(c), np.nan)
    for t in range(n - 1, len(c)):
        w = c[t - n + 1:t + 1]
        z[t] = (c[t] - w.mean()) / w.std(ddof=1)
    sig = U.build("X02", "S06", {"lookback": n, "entry_z": -1.5, "exit_z": 0.0, "trend": "none"})
    expect = np.nan_to_num(z, nan=1e9) <= -1.5
    expect[: int(np.flatnonzero(np.isfinite(z))[0])] = False
    assert np.array_equal(sig.cond, expect) and expect.any()


def test_s14_calendar_membership_comes_from_authority_not_prices(bars, U, U_perturbed):
    pb, boundary = U_perturbed
    c = next(c for c in ss.build_configs() if c["family"] == "S14")
    a = U.build("X03", c["family"], c["params"])
    b = sg.Universe(pb).build("X03", c["family"], c["params"])
    assert np.array_equal(a.d, b.d) and a.d.any()
    m = sg.calendar_member(U.sd["X03"], c["params"])
    assert np.array_equal(a.d[:-2], m[2:]) and not a.d[-2:].any() and np.array_equal(a.cond[:-1], m[1:])


def test_trend_and_cadence_refuse_unknown_values(U):
    with pytest.raises(ValueError):
        sg.trend_ok(U.sd["X03"], "sma100")
    with pytest.raises(ValueError):
        sg.cadence_anchor(U.sd["X03"], "weekly")
    with pytest.raises(ValueError):
        sg.calendar_member(U.sd["X03"], {"kind": "weekday"})


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


# ------------------------------------------------------------------------- ConditionalEdge factor authority (C2)

@pytest.fixture(scope="module")
def cctx():
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    universe = ss.build_universe(seed, {s: {"disposition": cdata.ELIGIBLE} for s in seed["symbols"]})
    bm = {"manifest_sha256": "0" * 64, "request_contract": cdata.REQUEST_CONTRACT}
    return cd.population_context(universe, ss.build_protocol(), ss.build_partitions(), bm)


def _cfg(family, **kw):
    for c in ss.build_configs():
        if c["family"] == family and all(c["params"].get(k) == v for k, v in kw.items()):
            return c
    raise KeyError((family, kw))


def _cond(family, **kw):
    for c in ss.build_conditions(ss.build_configs()):
        if c["family"] == family and all(c["params"].get(k) == v for k, v in kw.items()):
            return c
    raise KeyError((family, kw))


S05_CFG = dict(period=3, entry_below=20, exit_above=70, trend="none")
S05_COND = dict(period=3, entry_below=20, trend="none")


def test_declared_lookback_equals_first_defined_bar_for_every_config(U):
    for c in ss.build_configs():
        for s in U.symbols:
            sig = U.build(s, c["family"], c["params"])
            if sig is not None:
                assert sig.s == cd.declared_lookback(c), (c["family"], c["params"])


def test_ir7_every_conditional_candidate_is_a_registered_factorspec_with_zero_attempts(tmp_path, cctx):
    conds = ss.build_conditions(ss.build_configs())
    db = tmp_path / "r.sqlite"
    ids = cd.register_all_factors(db, conds, cctx)
    assert len(ids) == len(set(ids)) == 219 * 5 == len(cd.expected_factor_ids(conds, cctx))
    dig = cd.registered_population_digest(db, ids)
    assert dig["registered"] == 1095 and dig["attempts"] == 0
    horizons = {f["identity"]["horizon_periods"] for f in cd.list_factors(db, family=cd.FACTOR_FAMILY)}
    assert horizons == {1, 3, 5, 10, 20}
    with pytest.raises(RuntimeError, match="missing=1"):
        cd.registered_population_digest(db, ids + ["f" * 32])


def test_factor_identity_binds_semantics_but_not_layout_or_results(cctx):
    c = _cond("S05", **S05_COND)
    a = cd.factor_spec(c, 3, cctx)
    assert a.compute_factor_id() == cd.factor_spec(c, 3, cctx).compute_factor_id()
    assert a.compute_factor_id() != cd.factor_spec(c, 5, cctx).compute_factor_id()
    assert a.compute_factor_id() != cd.factor_spec(_cond("S05", **{**S05_COND, "entry_below": 30}), 3, cctx).compute_factor_id()
    other = {**cctx, "universe_identity": {**cctx["universe_identity"], "symbols_sha256": "x"}}
    assert a.compute_factor_id() != cd.factor_spec(c, 3, other).compute_factor_id()
    other = {**cctx, "data_provenance_identity": {**cctx["data_provenance_identity"], "bars_manifest_sha256": "1" * 64}}
    assert a.compute_factor_id() != cd.factor_spec(c, 3, other).compute_factor_id()
    with pytest.raises(ValueError):
        cd.factor_spec(c, 2, cctx)
    assert a.timing_convention == "same_bar_close_known_after_close" and a.information_lag_periods == 0


def test_ir8_ir9_results_and_retries_cannot_change_or_manufacture_a_factor(tmp_path, bars, U, cctx):
    c = _cond("S05", **S05_COND)
    db = tmp_path / "r.sqlite"
    ids = cd.register_all_factors(db, ss.build_conditions(ss.build_configs()), cctx)
    fid = cd.factor_spec(c, 3, cctx).compute_factor_id()
    r1 = cd.evaluate_factor(db, tmp_path / "o1", U, c, 3, cctx, origin="t")
    r2 = cd.evaluate_factor(db, tmp_path / "o2", sg.Universe(perturbed(bars, 900)), c, 3, cctx, origin="t")
    assert r1["factor_id"] == r2["factor_id"] == fid and r1["evaluation_id"] == r2["evaluation_id"]
    assert (r1["attempt_index"], r2["attempt_index"]) == (1, 2)
    assert r1["mean_ic"] != r2["mean_ic"] and r1["observations_content_sha256"] != r2["observations_content_sha256"]
    assert len(cd.list_factors(db, family=cd.FACTOR_FAMILY)) == 1095
    assert cd.expected_factor_ids(ss.build_conditions(ss.build_configs()), cctx) == ids
    assert len(cd.list_factor_evaluation_attempts(db, fid)) == 2


def test_ir28_registry_keeps_every_conditional_candidate_including_non_evaluable_ones(tmp_path, U, cctx):
    cfgs = ss.build_conditions(ss.build_configs())
    db = tmp_path / "r.sqlite"
    cd.register_all_factors(db, cfgs, cctx)
    s14 = [c for c in cfgs if c["family"] == "S14"][0]
    rec = cd.evaluate_factor(db, tmp_path / "o", U, s14, 1, cctx, origin="t")
    assert rec["status"] == "not_evaluable"  # a date-level condition is cross-sectionally constant
    assert cd.registered_population_digest(db, cd.expected_factor_ids(cfgs, cctx))["registered"] == 1095
    rep = cd.family_fdr_report(db, [])
    assert rep["status"] == "incomplete"  # 1094 factors never attempted: no decision on a partial population


def test_frame_label_is_diagnostic_future_return_minus_symbol_baseline_and_causal(U):
    fr, aux = cd.build_frame(U, _cond("S02", sma=50), 5)
    assert list(fr.columns) == cd.FRAME_COLUMNS and len(aux) == len(fr)
    assert (fr["information_cutoff_ts_utc"] <= fr["period_ts_utc"]).all()
    assert (fr["period_ts_utc"] < fr["label_end_ts_utc"]).all()
    for _sym, g in fr.groupby("symbol"):
        assert abs(g["label_fwd_ret"].mean()) < 1e-12
    assert set(fr["factor_value"].unique()) <= {0.0, 1.0}


def test_event_diagnostics_effect_is_conditional_minus_baseline_with_symbol_attribution(U):
    fr, aux = cd.build_frame(U, _cond("S02", sma=50), 1)
    ev = cd.event_diagnostics(fr, aux)
    m = fr["factor_value"].to_numpy() == 1.0
    assert ev["event_count"] == int(m.sum()) and ev["row_count"] == len(fr)
    assert ev["effect"] == pytest.approx(fr["label_fwd_ret"].to_numpy()[m].mean())
    assert sum(v["n"] for v in ev["per_symbol"].values()) == ev["event_count"] == sum(v["n"] for v in ev["per_year"].values())
    assert ev["symbols_represented"] == len(ev["per_symbol"]) and 0 < ev["top_symbol_event_share"] <= 1
    assert cd.event_diagnostics(fr.iloc[0:0], aux.iloc[0:0])["event_count"] == 0


def test_fast_empirical_pvalue_matches_native_repo_protocol_exactly(U):
    from mqk_research.factors.contracts import FactorEvaluationSpec
    from mqk_research.factors.diagnostics import evaluate_factor_ic_ir
    from mqk_research.factors.fdr import compute_empirical_pvalue
    fr, _aux = cd.build_frame(U, _cond("S05", **S05_COND), 3)
    kw = dict(n_quantiles=cd.N_QUANTILES, min_cross_section=cd.MIN_CROSS_SECTION, min_periods=cd.MIN_PERIODS)
    real = evaluate_factor_ic_ir(fr, **kw)
    spec = FactorEvaluationSpec(factor_id="f", universe_identity={}, evaluation_window_start_utc=cd.WINDOW_START_UTC,
                                evaluation_window_end_utc=cd.WINDOW_END_UTC, label_protocol_version="l",
                                evaluation_protocol_version="e")
    native = compute_empirical_pvalue(fr, spec, real, n_permutations=6, base_seed=0, **kw)
    fast = cd.fast_empirical_pvalue(fr, real.metrics, n_permutations=6, base_seed=0)
    assert {k: fast[k] for k in native} == native
    bad = json.loads(json.dumps(real.metrics))
    bad["per_period_ic"][next(iter(bad["per_period_ic"]))] += 0.01
    with pytest.raises(RuntimeError, match="fail closed"):
        cd.fast_empirical_pvalue(fr, bad, n_permutations=2)
    nb = fr.copy()
    nb.loc[nb.index[0], "factor_value"] = 0.5
    with pytest.raises(ValueError, match="binary"):
        cd.fast_empirical_pvalue(nb, real.metrics, n_permutations=2)


def test_population_freeze_covers_strategy_trials_with_zero_attempts(tmp_path):
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    syms = seed["symbols"][:2]
    uni = ss.build_universe(seed, {s: {"disposition": cdata.ELIGIBLE if s in syms else cdata.EXCLUDED_INSUFFICIENT_HISTORY}
                                   for s in seed["symbols"]})
    space = ss.build_search_space(ss.build_grammar(), uni, ss.build_partitions(), ss.build_protocol())
    _cfgs, _syms, _ids, cells = cs.population(uni, space)
    assert len(cells) == 434 * 2
    st = ResearchResultStore(tmp_path / "r.sqlite")
    with pytest.raises(cs.GateRefusal):
        cs.require_frozen_population(st, space, cells, allow_attempts=False)
    fz = cs.register_population(st, space, cells)
    assert fz["strategy_cell_count"] == 868 and set(fz) == {"search_space_id", "strategy_cell_count", "manifest_order_root",
                                                          "strategy_population_root"}
    proof = cs.require_frozen_population(st, space, cells, allow_attempts=False)
    assert proof["strategy_attempts"] == 0 and proof["registered_trials"] == 868
    st.begin_attempts_bulk([cells[0][3]], origin="x")
    with pytest.raises(cs.GateRefusal, match="attempts already exist"):
        cs.require_frozen_population(st, space, cells, allow_attempts=False)


# ------------------------------------------------------------- census gate / execution / resume (StrategyEdge)

@pytest.fixture(scope="module")
def mini(U, cctx):
    prot = ss.build_protocol()
    syms = list(U.symbols)
    cfgs = first_configs(2)
    space = {"search_space_id": "mini", "universe_id": "u" * 32, "partitions_id": "p" * 32, "protocol_id": "q" * 32}
    ids = {k: space[k] for k in ("universe_id", "partitions_id", "protocol_id")}
    index = {c["config_id"]: i for i, c in enumerate(cfgs)}
    cells = [(index[c["config_id"]], c, s, t) for c, s, t in ss.iter_cells(cfgs, syms, ids)]
    space["population_root_sha256"] = ss.population_root(cfgs, syms, ids)
    meta = {s: {"disposition": cs.DATA_PRESENT, "rows": len(U.sd[s].c), "data_short_history": s == syms[-1],
                "data_quality_caveat": False} for s in syms}
    return {"space": space, "cells": cells, "meta": meta, "prot": prot, "ctx": cctx}


def _fresh_store(tmp_path, mini, *, register=True):
    st = ResearchResultStore(tmp_path / "registry.sqlite")
    if register:
        cs.register_population(st, mini["space"], mini["cells"])
    return st


def _gate(st, mini, cells=None, **kw):
    return cs.require_frozen_population(st, mini["space"], cells or mini["cells"], **kw)


def _run(st, mini, U, out, **kw):
    return cs.run_chunks(st, U, mini["space"], mini["cells"], mini["meta"], out, **kw)


def test_gate_accepts_exact_frozen_population_with_zero_attempts(tmp_path, mini):
    r = _gate(_fresh_store(tmp_path, mini), mini, allow_attempts=False)
    assert r["registered_trials"] == len(mini["cells"]) > 100 and r["strategy_attempts"] == 0


def test_gate_refuses_missing_extra_duplicate_unfrozen_and_attempted(tmp_path, mini):
    cells = mini["cells"]
    st = ResearchResultStore(tmp_path / "a.sqlite")
    cs.register_population(st, mini["space"], cells[:-1])
    with pytest.raises(cs.GateRefusal, match="missing=1"):
        _gate(st, mini, allow_attempts=False)
    st = _fresh_store(tmp_path, mini)
    st.register_trials_bulk([{"trial_id": "ace2-extra", "experiment_id": ss.EXPERIMENT_ID,
                              "hypothesis_id": ss.hypothesis_id("S01"), "strategy_id": "x", "protocol_id": "q",
                              "identity": {"x": 1}}])
    with pytest.raises(cs.GateRefusal, match="extra=1"):
        _gate(st, mini, allow_attempts=True)
    with pytest.raises(cs.GateRefusal, match="duplicate"):
        cs.freeze_record(mini["space"], cells + [cells[0]])
    # trials registered but no freeze marker: an attempt before the population freeze is refused
    st = ResearchResultStore(tmp_path / "c.sqlite")
    for fam in {c[1]["family"] for c in cells}:
        st.register_hypothesis(hypothesis_id=ss.hypothesis_id(fam), experiment_id=ss.EXPERIMENT_ID, hypothesis_text="x")
    ids = {k: mini["space"][k] for k in ("universe_id", "partitions_id", "protocol_id")}
    st.register_trials_bulk([{"trial_id": t, "experiment_id": ss.EXPERIMENT_ID, "hypothesis_id": ss.hypothesis_id(c["family"]),
                              "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": ids["protocol_id"],
                              "identity": ss.trial_identity(c, s, ids)} for _i, c, s, t in cells])
    with pytest.raises(cs.GateRefusal, match="freeze marker"):
        _gate(st, mini, allow_attempts=True)
    # attempts != 0 at freeze time
    (tmp_path / "d").mkdir()
    st = _fresh_store(tmp_path / "d", mini)
    st.begin_attempts_bulk([cells[0][3]], origin="x")
    with pytest.raises(cs.GateRefusal, match="attempts already exist"):
        _gate(st, mini, allow_attempts=False)
    _gate(st, mini, allow_attempts=True)


def test_run_chunks_refuses_unregistered_population(tmp_path, mini, U):
    st = ResearchResultStore(tmp_path / "e.sqlite")
    with pytest.raises(cs.GateRefusal):
        _run(st, mini, U, tmp_path / "out", chunk_size=10)
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
    _run(st, mini, U, root, chunk_size=13)
    return root, st


def test_chunk_size_does_not_change_cell_economics_or_denominator(tmp_path, mini, U, reference_run):
    root, st = reference_run
    ref = _read(root)
    assert len(ref) == len(mini["cells"])
    st2 = _fresh_store(tmp_path, mini)
    _run(st2, mini, U, tmp_path, chunk_size=50)
    assert _read(tmp_path) == ref
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(v["attempts"] == 1 and v["succeeded"] == 1 for v in d.values()) and len(d) == len(mini["cells"])


def test_cell_lines_follow_manifest_order_and_contain_real_evaluations(mini, reference_run):
    rows = [json.loads(x) for x in _read(reference_run[0])]
    assert [r["t"] for r in rows] == [c[3] for c in mini["cells"]]
    ev = [r for r in rows if r["d"] == "EVALUABLE"]
    assert len(ev) > 100 and any(r["m"]["trade_count"] > 0 for r in ev)
    assert any(r["m"]["net_alpha_usd"] > 0 for r in ev), "fixture must contain positive-alpha cells (false-positive guard)"
    assert all("q" not in r for r in rows), "ConditionalEdge evidence never lives in the StrategyEdge ledger"


def test_interrupted_chunk_is_retried_as_new_attempt_with_identical_economics(tmp_path, mini, U, reference_run, monkeypatch):
    ref = _read(reference_run[0])
    st = _fresh_store(tmp_path, mini)
    real = cs.evaluate_cell
    n = {"i": 0}

    def flaky(*a, **k):
        n["i"] += 1
        if n["i"] == 20:
            raise RuntimeError("simulated infrastructure fault")
        return real(*a, **k)

    monkeypatch.setattr(cs, "evaluate_cell", flaky)
    with pytest.raises(RuntimeError, match="simulated"):
        _run(st, mini, U, tmp_path, chunk_size=13)
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    failed = [t for t, v in d.items() if v["failed"]]
    assert len(failed) == 13 and all(d[t]["started"] == 0 for t in d)
    assert len(list((tmp_path / "chunks").glob("chunk_*.jsonl"))) == 1
    monkeypatch.setattr(cs, "evaluate_cell", real)
    _run(st, mini, U, tmp_path, chunk_size=13)
    assert _read(tmp_path) == ref
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert len(d) == len(mini["cells"])  # a retry creates no new trial identity
    assert all(d[t]["attempts"] == 2 and d[t]["failed"] == 1 and d[t]["succeeded"] == 1 for t in failed)
    assert all(v["succeeded"] == 1 for v in d.values())


def test_crash_leftover_started_attempts_are_finalized_then_retried(tmp_path, mini, U, reference_run):
    st = _fresh_store(tmp_path, mini)
    part = mini["cells"][:13]
    st.begin_attempts_bulk([c[3] for c in part], origin="crashed")
    res = _run(st, mini, U, tmp_path, chunk_size=13)
    assert res["chunks_run"] == -(-len(mini["cells"]) // 13)
    d = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(d[c[3]]["failed"] == 1 and d[c[3]]["succeeded"] == 1 and d[c[3]]["started"] == 0 for c in part)
    assert _read(tmp_path) == _read(reference_run[0])


def test_resume_skips_terminal_chunks_and_is_idempotent(tmp_path, mini, U, reference_run):
    st = _fresh_store(tmp_path, mini)
    assert _run(st, mini, U, tmp_path, chunk_size=13, max_chunks=3)["chunks_run"] == 3
    assert _run(st, mini, U, tmp_path, chunk_size=13)["chunks_skipped_terminal"] == 3
    assert _run(st, mini, U, tmp_path, chunk_size=13)["chunks_run"] == 0
    after = st.trial_attempt_digest(ss.EXPERIMENT_ID)
    assert all(v["attempts"] == 1 for v in after.values())
    assert _read(tmp_path) == _read(reference_run[0])


def test_non_evaluable_symbol_stays_in_population(mini, U):
    meta = copy.deepcopy(mini["meta"])
    meta["X02"] = {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR"}
    r = cs.evaluate_cell(U, mini["cells"][0][1], "X02", meta)
    assert r == {"d": "NON_EVALUABLE_DATA_UNAVAILABLE_PROVIDER_ERROR", "m": None}


# ------------------------------------------------------------- C3: qualification taxonomy + Edge Registry (IR10-IR20, IR24, IR25, IR27)

import edge_registry as er  # noqa: E402

REAL_M = {"window_bars": 500, "round_trips": 9, "trade_count": 10, "net_pnl_usd": 120.0, "net_alpha_usd": 40.0,
          "cost_usd": 30.0, "benchmark_net_pnl_usd": 80.0, "benchmark_cost_usd": 4.0, "sharpe": None,
          "year_alpha_usd": {"2020": 1.0}}
JUDGE_OK = {"status": "COMPLETE", "population_complete": True, "dsr": 0.7, "pbo": 0.3}


def _m(**kw):
    return {**REAL_M, **kw}


def test_ir10_positive_alpha_with_non_positive_net_pnl_is_not_a_record():
    for pnl in (-0.01, 0.0, -500.0):
        assert er.strategy_class("EVALUABLE", _m(net_pnl_usd=pnl, net_alpha_usd=900.0)) is None
    assert er.strategy_positive_below_floor("EVALUABLE", _m(net_pnl_usd=-5.0, net_alpha_usd=900.0), None)


@pytest.mark.parametrize("trips,cls", [(0, None), (4, None), (5, er.MODERATE)])
def test_ir11_fewer_than_five_closed_round_trips_is_not_a_record(trips, cls):
    assert er.strategy_class("EVALUABLE", _m(round_trips=trips)) == cls
    assert er.strategy_class("EVALUABLE", _m(round_trips=trips, trade_count=99)) == cls  # open runs never count


@pytest.mark.parametrize("bars_,cls", [(251, None), (252, er.MODERATE)])
def test_ir12_fewer_than_252_evaluated_bars_is_not_a_record(bars_, cls):
    assert er.strategy_class("EVALUABLE", _m(window_bars=bars_)) == cls


def test_ir13_positive_net_pnl_with_floors_is_weak_even_when_alpha_is_not_positive():
    for alpha in (0.0, -10.0):
        assert er.strategy_class("EVALUABLE", _m(net_alpha_usd=alpha)) == er.WEAK


def test_ir14_weak_plus_positive_finite_matched_benchmark_alpha_is_moderate():
    assert er.strategy_class("EVALUABLE", _m(net_alpha_usd=0.01)) == er.MODERATE
    assert er.strategy_class("EVALUABLE", _m(net_alpha_usd=float("nan"))) is None


@pytest.mark.parametrize("judge,cls", [
    (None, er.MODERATE),
    ({"status": "DEFERRED_FULL_POPULATION"}, er.MODERATE),
    ({**JUDGE_OK, "population_complete": False}, er.MODERATE),
    ({**JUDGE_OK, "dsr": 0.49}, er.MODERATE),
    ({**JUDGE_OK, "pbo": 0.51}, er.MODERATE),
    ({**JUDGE_OK, "dsr": None}, er.MODERATE),
    (JUDGE_OK, er.STRONG),
])
def test_ir15_no_strong_while_the_full_population_judge_is_deferred_or_unmet(judge, cls):
    assert er.strategy_class("EVALUABLE", _m(), judge) == cls
    assert ss.JUDGE_STATUS == "DEFERRED_FULL_POPULATION"


@pytest.mark.parametrize("drop", ["cost_usd", "benchmark_net_pnl_usd", "benchmark_cost_usd", "net_pnl_usd", "net_alpha_usd",
                                  "window_bars", "round_trips"])
def test_strategy_edge_needs_cost_benchmark_and_finite_metrics(drop):
    assert er.strategy_class("EVALUABLE", {k: v for k, v in _m().items() if k != drop}) is None
    assert er.strategy_class("EVALUABLE", _m(**{drop: None})) is None
    assert er.strategy_class("EVALUABLE", _m(**{drop: float("inf")})) is None
    assert er.strategy_class("EVALUABLE", _m(year_alpha_usd={"2020": float("nan")})) is None


def test_non_evaluable_cells_are_never_records():
    assert er.strategy_class("NON_EVALUABLE_SIGNAL_NOT_DEFINED", None) is None
    assert er.strategy_class("NON_EVALUABLE_X", _m()) is None


def _rec(n=30, eff=0.002, p=0.05, status="succeeded", fid="f1", **kw):
    return {"factor_id": fid, "evaluation_id": "e1", "attempt_index": 1, "status": status, "mean_ic": 0.01,
            "events": {"event_count": n, "direction_adjusted_effect": eff, "symbols_represented": 5,
                       "top_symbol_event_share": 0.3, "per_year": {}, "per_regime": {}},
            "pvalue": {"p_value": p}, **kw}


FDR_OK = {"status": "complete", "q_values": {"f1": 0.05}}


def test_ir16_conditional_with_29_events_never_enters_and_ir17_30_events_positive_effect_is_weak():
    assert er.conditional_class(_rec(n=29, p=0.001), FDR_OK) is None
    assert er.conditional_class(_rec(n=30, p=0.5), FDR_OK) == er.WEAK
    for eff in (0.0, -0.001, float("nan"), None):
        assert er.conditional_class(_rec(eff=eff, p=0.001), FDR_OK) is None
    assert er.conditional_class(_rec(status="not_evaluable"), FDR_OK) is None
    assert er.conditional_class(_rec(status="failed"), FDR_OK) is None
    assert er.conditional_positive_below_floor(_rec(n=29), None)


@pytest.mark.parametrize("p,cls", [(0.10, er.STRONG), (0.1000001, er.WEAK), (0.5, er.WEAK), (None, er.WEAK)])
def test_ir18_moderate_needs_the_empirical_null_p_at_most_point_ten(p, cls):
    rec = _rec(p=p)
    rec["pvalue"] = None if p is None else {"p_value": p}
    got = er.conditional_class(rec, {"status": "complete", "q_values": {"f1": 0.10}})
    assert got == cls
    assert er.conditional_class(rec, None) == (er.MODERATE if cls == er.STRONG else er.WEAK)


@pytest.mark.parametrize("fdr,cls", [
    (None, er.MODERATE),
    ({"status": "incomplete", "q_values": {"f1": 0.01}}, er.MODERATE),
    ({"status": "not_evaluable", "q_values": None}, er.MODERATE),
    ({"status": "complete", "q_values": {"f1": 0.1001}}, er.MODERATE),
    ({"status": "complete", "q_values": {}}, er.MODERATE),
    ({"status": "complete", "q_values": {"f1": 0.10}}, er.STRONG),
])
def test_ir19_strong_needs_a_complete_full_family_fdr_and_q_at_most_point_ten(fdr, cls):
    assert er.conditional_class(_rec(p=0.01), fdr) == cls
    assert ss.DISCOVERY_FDR_ALPHA == 0.10


def test_authoritative_factor_record_is_highest_attempt_not_the_best_result():
    good, bad = _rec(p=0.001, attempt_index=1), _rec(attempt_index=2, status="failed")
    assert er.authoritative_factor_records([good, bad])["f1"]["status"] == "failed"
    assert er.authoritative_factor_records([bad, good])["f1"]["status"] == "failed"


def test_edge_id_is_result_independent_and_kind_separated():
    assert er.edge_id("STRATEGY_EDGE", "t1") == er.edge_id("STRATEGY_EDGE", "t1")
    assert er.edge_id("STRATEGY_EDGE", "t1") != er.edge_id("CONDITIONAL_EDGE", "t1") != er.edge_id("STRATEGY_EDGE", "t2")


def test_neighbor_map_adjacent_numeric_only():
    cfgs = [{"family": "F", "params": {"a": a, "b": b, "m": "x"}} for a in (1, 2, 4) for b in (10, 20)]
    nb = er.neighbor_map(cfgs)
    idx = {(c["params"]["a"], c["params"]["b"]): i for i, c in enumerate(cfgs)}
    assert set(nb[idx[(2, 10)]]) == {idx[(1, 10)], idx[(4, 10)], idx[(2, 20)]}
    assert set(nb[idx[(1, 10)]]) == {idx[(2, 10)], idx[(1, 20)]}
    assert er.neighbor_map([{"family": "G", "params": {"a": 2}}, {"family": "F", "params": {"a": 3}}]) == [[], []]


def test_ir25_label_return_cannot_reach_strategy_pnl(U, mini, monkeypatch):
    import inspect
    assert "label" not in inspect.getsource(cs.evaluate_cell) and "fwd_ret" not in inspect.getsource(cs.evaluate_cell)
    assert not any("label" in p or "fwd" in p for p in inspect.signature(sm.simulate).parameters)
    cfg, sym = mini["cells"][0][1], mini["cells"][0][2]
    ref = cs.evaluate_cell(U, cfg, sym, mini["meta"])

    def boom(*a, **k):
        raise AssertionError("StrategyEdge economics must not touch the conditional label frame")

    monkeypatch.setattr(cd, "build_frame", boom)
    assert cs.evaluate_cell(U, cfg, sym, mini["meta"]) == ref


def test_ir24_rejected_run_population_and_attempts_cannot_satisfy_the_corrected_freeze_gate(tmp_path, mini):
    cells = mini["cells"]
    assert ss.REJECTED_EXPERIMENT_ID != ss.EXPERIMENT_ID
    st = ResearchResultStore(tmp_path / "rej.sqlite")
    ids = {k: mini["space"][k] for k in ("universe_id", "partitions_id", "protocol_id")}
    st.register_hypothesis(hypothesis_id="rej-h", experiment_id=ss.REJECTED_EXPERIMENT_ID, hypothesis_text="x")
    st.register_trials_bulk([{"trial_id": t, "experiment_id": ss.REJECTED_EXPERIMENT_ID, "hypothesis_id": "rej-h",
                              "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": ids["protocol_id"],
                              "identity": ss.trial_identity(c, s, ids)} for _i, c, s, t in cells])
    st.begin_attempts_bulk([cells[0][3]], origin="rejected")
    with pytest.raises(cs.GateRefusal, match="missing="):
        _gate(st, mini, allow_attempts=True)
    with pytest.raises(cs.GateRefusal, match="missing="):
        _gate(st, mini, allow_attempts=False)


# ---- end-to-end registry over a small real population (registered factors, real runner, real FDR)

@pytest.fixture(scope="module")
def reg_run(tmp_path_factory, U, cctx):
    root = tmp_path_factory.mktemp("reg")
    cfgs = [_cfg("S02", sma=50), _cfg("S05", **S05_CFG), _cfg("S05", **{**S05_CFG, "entry_below": 30}),
            [c for c in ss.build_configs() if c["family"] == "S14"][0]]
    conds = ss.build_conditions(cfgs)
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    base = ss.build_universe(seed, {s: {"disposition": cdata.ELIGIBLE} for s in seed["symbols"]})
    uni = {**base, "symbols": list(U.symbols)}
    ids = {"universe_id": "u" * 32, "partitions_id": "p" * 32, "protocol_id": "q" * 32}
    space = {"search_space_id": "reg", **ids}
    index = {c["config_id"]: i for i, c in enumerate(cfgs)}
    cells = [(index[c["config_id"]], c, s, t) for c, s, t in ss.iter_cells(cfgs, list(U.symbols), ids)]
    space["population_root_sha256"] = ss.population_root(cfgs, list(U.symbols), ids)
    meta = {s: {"disposition": cs.DATA_PRESENT, "rows": len(U.sd[s].c), "data_short_history": False,
                "data_quality_caveat": False} for s in U.symbols}
    st = ResearchResultStore(root / "registry.sqlite")
    cs.register_population(st, space, cells)
    cs.run_chunks(st, U, space, cells, meta, root, chunk_size=20)
    fst = ResearchResultStore(root / "registry_conditional_v3.sqlite")
    cd.register_all_factors(fst.db_path, conds, cctx)
    cache = cd.PermutationCache()
    recs = [cd.evaluate_factor(fst.db_path, root / "fac", U, c, h, cctx, origin="t", cache=cache)
            for c in conds for h in ss.CONDITIONAL_HORIZONS]
    fdr = cd.family_fdr_report(fst.db_path, recs)
    prot = {**ss.build_protocol(), "chunking": {"cells_per_chunk": 20}}
    out = root / "out"
    out.mkdir()
    import shutil
    shutil.copytree(root / "chunks", out / "chunks")
    summary = er.build_registry(space, uni, cells, meta, prot, out, ctx=cctx, factor_records=recs, fdr=fdr)
    return {"root": root, "out": out, "space": space, "uni": uni, "cells": cells, "meta": meta, "prot": prot,
            "recs": recs, "fdr": fdr, "summary": summary, "ctx": cctx, "cfgs": cfgs, "conds": conds, "st": st,
            "fst": fst}


def _lines(p):
    return [json.loads(x) for x in Path(p).read_text(encoding="utf-8").splitlines()]


def _edges(r):
    return _lines(r["out"] / er.OUT_EDGES)


def test_registry_records_exactly_the_qualifying_candidates_at_their_highest_class(reg_run):
    r = reg_run
    rows = [json.loads(x) for x in _read(r["root"])]
    exp_s = {x["t"]: er.strategy_class(x["d"], x["m"]) for x in rows}
    exp_s = {t: c for t, c in exp_s.items() if c}
    auth = er.authoritative_factor_records(r["recs"])
    exp_c = {f: er.conditional_class(a, r["fdr"]) for f, a in auth.items()}
    exp_c = {f: c for f, c in exp_c.items() if c}
    edges = _edges(r)
    got_s = {e["trial_id"]: e["edge_class"] for e in edges if e["kind"] == "STRATEGY_EDGE"}
    got_c = {e["factor_id"]: e["edge_class"] for e in edges if e["kind"] == "CONDITIONAL_EDGE"}
    assert got_s == exp_s and got_c == exp_c
    assert exp_s and exp_c, "fixture must contain both qualifying StrategyEdges and ConditionalEdges"
    assert len(got_s) < len(rows) and len(got_c) < len(auth), "fixture must contain non-qualifying candidates"
    assert len({e["edge_id"] for e in edges}) == len(edges)
    for e in edges:
        assert e["edge_class"] in (er.WEAK, er.MODERATE, er.STRONG)
        assert sum(k in e for k in ("edge_class",)) == 1
    s = r["summary"]
    assert sum(s["strategy_edges"].values()) == len(got_s) and sum(s["conditional_edges"].values()) == len(got_c)
    assert s["strategy_edges"][er.STRONG] == 0 and s["JUDGE_STATUS"] == "DEFERRED_FULL_POPULATION"


def test_every_record_is_not_validated_no_promotion_non_executable_and_survivorship_labeled(reg_run):
    for e in _edges(reg_run):
        assert e["VALIDATION_STATUS"] == "NOT_VALIDATED" and e["PROMOTION_AUTHORITY"] == "NONE"
        assert e["label"] == "DISCOVERED / NOT VALIDATED" and e["partition"] == "DISCOVERY_2016_2023"
        assert e["survivorship_classification"] == "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"
        assert e["point_in_time_membership"] is False and "survivorship_caveat" in e["flags"]
        if e["kind"] == "CONDITIONAL_EDGE":
            assert e["executable_pnl"] is False and "metrics" not in e
            assert e["horizon"] in (1, 3, 5, 10, 20) and e["events"]["event_count"] >= 30
        else:
            assert e["metrics"]["net_pnl_usd"] > 0 and e["metrics"]["round_trips"] >= 5 and e["metrics"]["window_bars"] >= 252
    s = reg_run["summary"]
    assert s["PROMOTION_AUTHORITY"] == "NONE" and s["VALIDATION_STATUS"] == "NOT_VALIDATED"


def test_ir27_search_ledger_keeps_every_strategy_candidate_including_losers_and_below_floor(reg_run):
    r = reg_run
    led = _lines(r["out"] / er.OUT_SEARCH_LEDGER)
    assert [x["t"] for x in led] == [c[3] for c in r["cells"]]
    s = r["summary"]
    assert len(led) == s["cells_total"] == len(r["cells"]) > sum(s["strategy_edges"].values())
    assert any(x.get("net_pnl_usd", 1) <= 0 for x in led), "losing candidates must stay in the denominator"
    assert s["strategy_positive_below_floor"] == sum(1 for x in led if x.get("below_floor"))
    assert all(x["class"] is None for x in led if x.get("below_floor")), "below-floor is never a registry class"
    assert sum(s["dispositions"].values()) == len(r["cells"])


def test_ir28_factor_ledger_keeps_every_registered_factor_including_non_evaluable(reg_run):
    r = reg_run
    fl = _lines(r["out"] / er.OUT_FACTOR_LEDGER)
    s = r["summary"]
    assert len(fl) == s["factors_total"] == len(r["conds"]) * 5 == len(r["fdr"]["declared_factor_ids"])
    assert {x["status"] for x in fl} >= {"succeeded", "not_evaluable"}
    assert any(x["class"] is None for x in fl)
    assert sum(s["conditional_edges"].values()) < len(fl)
    assert s["conditional_positive_below_floor"] == sum(1 for x in fl if x["below_floor"])


def test_ir20_fdr_population_is_the_registry_and_keeps_non_evaluable_and_negative_factors(reg_run):
    r = reg_run
    fdr = r["fdr"]
    assert fdr["status"] == "complete" and len(fdr["declared_factor_ids"]) == len(r["conds"]) * 5
    assert fdr["excluded_factor_ids_with_reasons"], "non-evaluable (date-level) factors stay accounted as typed exclusions"
    neg = [a for a in r["recs"] if a["status"] == "succeeded" and a["events"]["direction_adjusted_effect"] is not None
           and a["events"]["direction_adjusted_effect"] <= 0]
    assert neg and all(a["factor_id"] in fdr["raw_p_values"] and a["factor_id"] in fdr["q_values"] for a in neg)
    assert set(fdr["included_factor_ids"]) | set(fdr["excluded_factor_ids_with_reasons"]) == set(fdr["declared_factor_ids"])


def test_registry_refuses_fdr_built_from_winners_or_a_partial_factor_record_set(reg_run, tmp_path):
    r = reg_run
    auth = er.authoritative_factor_records(r["recs"])
    winners = [x for x in r["recs"] if er.conditional_class(auth[x["factor_id"]], r["fdr"])]
    assert winners and len(winners) < len(r["recs"])
    kw = dict(ctx=r["ctx"])
    out = tmp_path / "o"
    out.mkdir()
    import shutil
    shutil.copytree(r["out"] / "chunks", out / "chunks")
    args = (r["space"], r["uni"], r["cells"], r["meta"], r["prot"], out)
    with pytest.raises(er.RegistryRefusal, match="factor records != registered population"):
        er.build_registry(*args, factor_records=winners, fdr=r["fdr"], **kw)
    narrowed = {**r["fdr"], "declared_factor_ids": [x["factor_id"] for x in winners]}
    with pytest.raises(er.RegistryRefusal, match="FDR population differs"):
        er.build_registry(*args, factor_records=r["recs"], fdr=narrowed, **kw)
    assert cd.family_fdr_report(r["fst"].db_path, winners)["declared_factor_ids"].__len__() == len(r["recs"])


def test_registry_is_deterministic_and_hashes_bind_files(reg_run):
    import hashlib
    r = reg_run
    s = r["summary"]
    for key, name in (("edge_registry_sha256", er.OUT_EDGES), ("search_ledger_sha256", er.OUT_SEARCH_LEDGER),
                      ("factor_ledger_sha256", er.OUT_FACTOR_LEDGER)):
        assert hashlib.sha256((r["out"] / name).read_bytes()).hexdigest() == s[key]
    again = er.build_registry(r["space"], r["uni"], r["cells"], r["meta"], r["prot"], r["out"], ctx=r["ctx"],
                              factor_records=list(reversed(r["recs"])), fdr=r["fdr"])
    assert again == s


def test_ids_do_not_depend_on_results_or_layout(reg_run):
    r = reg_run
    for e in _edges(r):
        ident = e["trial_id"] if e["kind"] == "STRATEGY_EDGE" else e["factor_id"]
        assert e["edge_id"] == er.edge_id(e["kind"], ident)


def test_neighborhood_flags_never_delete_island_edges(reg_run):
    edges = _edges(reg_run)
    s = reg_run["summary"]
    assert s["parameter_island_edges"] == sum("parameter_island" in e["flags"] for e in edges)
    assert len(edges) == sum(s["strategy_edges"].values()) + sum(s["conditional_edges"].values())


def test_registry_refuses_tampered_or_missing_chunk(tmp_path, reg_run):
    import shutil
    r = reg_run
    root = tmp_path / "t"
    shutil.copytree(r["out"], root)
    args = (r["space"], r["uni"], r["cells"], r["meta"], r["prot"], root)
    kw = dict(ctx=r["ctx"], factor_records=r["recs"], fdr=r["fdr"])
    p0 = er.chunk_path(root, 0)
    lines = p0.read_text(encoding="utf-8").splitlines()
    p0.write_text("\n".join(lines[1:] + lines[:1]) + "\n", encoding="utf-8")
    with pytest.raises(er.RegistryRefusal, match="not the manifest cell"):
        er.build_registry(*args, **kw)
    p0.write_text("\n".join(lines[:-1]) + "\n", encoding="utf-8")
    with pytest.raises(er.RegistryRefusal, match="line count"):
        er.build_registry(*args, **kw)
    p0.unlink()
    with pytest.raises(er.RegistryRefusal, match="missing chunk"):
        er.build_registry(*args, **kw)


# ---- bars manifest / symbol meta

def _fake_bars_dir(tmp_path, symbols, absent=()):
    for sym in symbols:
        d = tmp_path / sym
        d.mkdir()
        status = {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR" if sym in absent else cs.DATA_PRESENT}
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
        return bars[sym_dir.name].copy(), {"artifact_sha256": "a-" + sym_dir.name,
                                           "canonical_semantic_bars_hash": state["hash"] + sym_dir.name,
                                           "canonical_pricing_bars_hash": "p"}

    monkeypatch.setattr(cs, "load_symbol_bars", fake_load)
    m = cs.build_bars_manifest(uni, tmp_path)
    assert set(m["symbols"]) == set(syms)
    assert m["symbols"]["X01"] == {"disposition": "DATA_UNAVAILABLE_PROVIDER_ERROR"}
    assert m["symbols"]["SPY"]["rows"] == len(bars["SPY"]) and m["symbols"]["SPY"]["provenance_file_sha256"]
    assert m["request_contract"] == cdata.REQUEST_CONTRACT
    assert cs.load_bars(uni, tmp_path, m).keys() == {"SPY", "X00"}
    state["hash"] = "h2"
    with pytest.raises(cs.GateRefusal, match="differ"):
        cs.load_bars(uni, tmp_path, m)
    assert cs.build_bars_manifest(uni, tmp_path)["manifest_sha256"] != m["manifest_sha256"]


def test_symbol_meta_flags_short_history_and_zero_volume_caveat():
    prot = ss.build_protocol()
    man = {"symbols": {"A": {"disposition": cs.DATA_PRESENT, "rows": 1499, "zero_volume_bars": 20},
                       "B": {"disposition": cs.DATA_PRESENT, "rows": 1500, "zero_volume_bars": 19},
                       "C": {"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION"}}}
    meta = cs.symbol_meta(man, prot)
    assert meta["A"]["data_short_history"] and meta["A"]["data_quality_caveat"]
    assert not meta["B"]["data_short_history"] and not meta["B"]["data_quality_caveat"]
    assert meta["C"] == {"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION"}


def test_c18_candidate_ids_do_not_depend_on_list_position_shard_or_layout():
    cfgs = first_configs(2)
    ids = {"universe_id": "u" * 32, "partitions_id": "p" * 32, "protocol_id": "q" * 32}
    full = {(c["config_id"], s): t for c, s, t in ss.iter_cells(cfgs, ["B", "A", "C"], ids)}
    sub = {(c["config_id"], s): t for c, s, t in ss.iter_cells(list(reversed(cfgs)), ["C", "A"], ids)}
    assert sub and all(full[k] == v for k, v in sub.items())
    assert len(set(full.values())) == len(full) == len(cfgs) * 3
    assert er.edge_id("STRATEGY_EDGE", "t1") == ss.sha256_canonical({"kind": "STRATEGY_EDGE", "id": "t1"})[:32]


# ---- committed population freeze proof (C5): bound to the committed manifests, zero attempts, 434 x eligible

def _norm_sha(p):
    import hashlib
    return hashlib.sha256(Path(p).read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def test_population_freeze_proof_binds_committed_manifests_and_counts():
    proof = _load("POPULATION_FREEZE_PROOF_V2.json")
    uni, space, grammar = _load("ALPHA_CENSUS_UNIVERSE_V2.json"), _load("ALPHA_CENSUS_SEARCH_SPACE_V2.json"), _load("ALPHA_CENSUS_GRAMMAR_V2.json")
    seed = _load("ALPHA_CENSUS_SEED_UNIVERSE_V2.json")
    assert proof["strategy_edge_config_count"] == len(grammar["configs"]) == 434
    assert proof["per_family_config_counts"] == ss.EXPECTED_FAMILY_COUNTS and proof["family_set"] == list(ss.EXPECTED_FAMILY_IDS)
    assert sorted(uni["dispositions"]) == seed["symbols"] and proof["seed_symbol_count"] == len(seed["symbols"])
    assert proof["eligible_symbol_count"] == len(uni["symbols"]) == uni["symbol_count"]
    assert all(r["disposition"] == "ELIGIBLE" or r["disposition"] in cdata.EXCLUDED_DISPOSITIONS
               for r in uni["dispositions"].values())
    assert proof["derived_strategy_edge_trial_count"] == 434 * proof["eligible_symbol_count"] == space["strategy_trial_count"]
    assert proof["registered_strategy_trials"] == space["strategy_trial_count"]
    assert proof["registered_factors"] == proof["conditional_factor_count"] == 434 * 5 == space["conditional_factor_count"]
    assert proof["strategy_attempts"] == proof["factor_evaluation_attempts"] == proof["attempts_at_freeze"] == 0
    assert pd.Timestamp(proof["max_economic_input_end_ts"]) <= pd.Timestamp("2023-12-29T23:59:59", tz="UTC")
    assert proof["partition_ids"]["partitions_sha256"] == ss.sha256_canonical(ss.build_partitions())
    assert proof["population_hashes"]["strategy_population_root"] and proof["population_hashes"]["conditional_factor_population_root"]
    for name, digest in proof["manifest_file_sha256"].items():
        assert _norm_sha(EXP / name) == digest, name


# ------------------------------------------------------------------ V3 semantic ConditionalEdge authority (CR-01)

CONDS = ss.build_conditions(ss.build_configs())
EXEC_SEAMS = [("S04", "exit"), ("S05", "exit_above"), ("S07", "hold"), ("S08", "hold"), ("S09", "exit"), ("S11", "hold"),
              ("S12", "hold"), ("S13", "hold")]


def _pairs_differing_only_in(configs, family, key):
    out = []
    fam = [c for c in configs if c["family"] == family]
    for i, a in enumerate(fam):
        for b in fam[i + 1:]:
            diff = {k for k in set(a["params"]) | set(b["params"]) if a["params"].get(k) != b["params"].get(k)}
            if diff == {key}:
                out.append((a, b))
    return out


@pytest.mark.parametrize("family,count", sorted(ss.EXPECTED_CONDITION_FAMILY_COUNTS.items()))
def test_d1_semantic_condition_count_per_family(family, count):
    assert sum(1 for c in CONDS if c["family"] == family) == count


def test_d1_semantic_population_is_219_conditions_and_1095_factors(cctx):
    assert len(CONDS) == ss.EXPECTED_CONDITION_COUNT == 219 and sum(ss.EXPECTED_CONDITION_FAMILY_COUNTS.values()) == 219
    assert len(ss.build_configs()) == 434
    ss.assert_condition_authority(CONDS, ss.build_configs())
    ids = cd.expected_factor_ids(CONDS, cctx)
    assert len(ids) == len(set(ids)) == ss.EXPECTED_CONDITIONAL_FACTOR_COUNT == 1095
    g = ss.build_condition_grammar(ss.build_configs())
    assert g["condition_count"] == 219 and g["conditional_factor_count"] == 1095
    rev = ss.build_condition_grammar(list(reversed(ss.build_configs())))
    assert rev["condition_grammar_id"] == g["condition_grammar_id"], "grammar id is independent of config order"


def test_d1_projection_partitions_params_and_maps_every_strategy_config_exactly_once():
    cfgs = ss.build_configs()
    for f in ss.EXPECTED_FAMILY_IDS:
        keys = {k for c in cfgs if c["family"] == f for k in c["params"]}
        cond_keys, exec_keys = set(ss.CONDITION_PARAM_KEYS[f]), set(ss.EXECUTION_ONLY_KEYS.get(f, ()))
        assert not cond_keys & exec_keys and keys <= cond_keys | exec_keys and exec_keys <= keys, f
    mapped = [cid for c in CONDS for cid in c["source_config_ids"]]
    assert sorted(mapped) == sorted(c["config_id"] for c in cfgs) and len(mapped) == 434
    assert all(c["source_config_ids"] == sorted(c["source_config_ids"]) for c in CONDS)
    assert max(len(c["source_config_ids"]) for c in CONDS) > 1, "fixture must contain duplicate-collapsing groups"
    for c in CONDS:
        assert c["condition_id"] == ss.condition_id(c["family"], c["params"])
    with pytest.raises(ss.GrammarRefusal, match="neither condition-defining nor execution-only"):
        ss.condition_params("S04", {"entry": 20, "exit": 10, "surprise": 1})
    with pytest.raises(ss.GrammarRefusal, match="execution-only parameters"):
        ss.condition_params("S07", {"decline_sessions": 1, "atr_window": 14, "mult": 1.0, "trend": "none"})


@pytest.mark.parametrize("family,key", EXEC_SEAMS)
def test_d1_execution_only_param_changes_cannot_mint_a_distinct_condition_or_factor(family, key, cctx):
    pairs = _pairs_differing_only_in(ss.build_configs(), family, key)
    assert pairs, (family, key)
    for a, b in pairs:
        ca, cb = ss.condition_params(family, a["params"]), ss.condition_params(family, b["params"])
        assert ca == cb and ss.condition_id(family, ca) == ss.condition_id(family, cb)
    cond = next(c for c in CONDS if c["family"] == family and len(c["source_config_ids"]) > 1)
    assert len({spec.compute_factor_id() for _c, _h, spec in cd.iter_factor_specs([cond], cctx)}) == len(ss.CONDITIONAL_HORIZONS)


def gappy_bars(symbol: str, seed: int) -> pd.DataFrame:
    """Event-rich fixture: recurring gap-down opens so that the S08 gap event genuinely occurs (the plain synthetic
    bars almost never gap, which would make an S08 equivalence check vacuous)."""
    df = synth_bars(symbol, seed, phi=-0.2, sig=0.015)
    rng = np.random.default_rng(seed)
    o, c = df["open"].to_numpy().copy(), df["close"].to_numpy()
    lo, hi = df["low"].to_numpy().copy(), df["high"].to_numpy().copy()
    for i in range(30, len(df), 9):
        o[i] = round(c[i - 1] * (1 - rng.uniform(0.02, 0.08)), 2)
        lo[i] = min(lo[i], o[i], c[i])
        hi[i] = max(hi[i], o[i], c[i])
    return df.assign(open=o, low=lo, high=hi)


@pytest.fixture(scope="module")
def UG(bars):
    return sg.Universe({"SPY": bars["SPY"], "G0": gappy_bars("G0", 501), "G1": gappy_bars("G1", 502)})


@pytest.mark.parametrize("family,key", EXEC_SEAMS)
def test_d1_execution_only_change_leaves_condition_series_identical_but_changes_the_strategy_signal(family, key, U, UG):
    changed_d = False
    for UU in (U, UG):
        for a, b in _pairs_differing_only_in(ss.build_configs(), family, key):
            for sym in UU.symbols:
                sa, sb = UU.build(sym, family, a["params"]), UU.build(sym, family, b["params"])
                assert (sa is None) == (sb is None)
                if sa is None:
                    continue
                assert sa.s == sb.s and np.array_equal(sa.cond, sb.cond), (family, key, sym)
                changed_d |= not np.array_equal(sa.d, sb.d)
    assert changed_d, f"{family}.{key}: the execution-only parameter must really change d (else the equivalence is vacuous)"


def test_d1_condition_series_equals_every_source_strategy_config_series(U, UG):
    proof = cd.condition_equivalence_proof(U, ss.build_configs(), CONDS)
    assert proof["conditions"] == 219 and proof["mismatches"] == 0 and proof["multi_source_conditions"] > 20
    assert proof["source_config_series_compared"] == 434 * len(U.symbols)
    gproof = cd.condition_equivalence_proof(UG, ss.build_configs(), CONDS)
    assert gproof["mismatches"] == 0 and gproof["source_config_series_compared"] == 434 * len(UG.symbols)
    assert gproof == cd.condition_equivalence_proof(UG, ss.build_configs(), CONDS)
    bad = [{**c, "params": {**c["params"], "entry": 50}} if c["family"] == "S04" and c["params"]["entry"] == 20 else c
           for c in CONDS]
    with pytest.raises(cd.ConditionEquivalenceError):
        cd.condition_equivalence_proof(U, ss.build_configs(), bad)


def test_d1_condition_lookback_equals_first_defined_bar_for_every_condition(U):
    for c in CONDS:
        for s in U.symbols:
            sig = cd.condition_sig(U, s, c)
            if sig is not None:
                assert sig.s == cd.condition_lookback(c["family"], c["params"]), (c["family"], c["params"])


def test_d1_genuine_condition_params_do_mint_distinct_identities_and_observations(U, UG, cctx):
    no_pair = set()
    for f in ss.EXPECTED_FAMILY_IDS:
        for key in ss.CONDITION_PARAM_KEYS[f]:
            pairs = _pairs_differing_only_in(CONDS, f, key)
            if not pairs:
                no_pair.add((f, key))
                continue
            a, b = pairs[0]
            assert a["condition_id"] != b["condition_id"]
            assert cd.factor_spec(a, 5, cctx).compute_factor_id() != cd.factor_spec(b, 5, cctx).compute_factor_id()
            differs = False
            for UU in (U, UG):
                for sym in UU.symbols:
                    sa, sb = cd.condition_sig(UU, sym, a), cd.condition_sig(UU, sym, b)
                    differs |= (sa is None) != (sb is None) or (sa is not None and (sa.s != sb.s or not np.array_equal(sa.cond, sb.cond)))
            assert differs, (f, key)
    assert no_pair <= {("S13", "long_vol"), ("S14", "kind"), ("S14", "value"), ("S14", "last"), ("S14", "first")}, no_pair


def test_d1_factor_spec_refuses_execution_only_param_or_forged_condition_id(cctx):
    c = _cond("S07", decline_sessions=1)
    leaky = {**c, "params": {**c["params"], "hold": 3}}
    with pytest.raises(ValueError, match="execution-only"):
        cd.factor_spec(leaky, 1, cctx)
    with pytest.raises(ValueError, match="condition_id does not match"):
        cd.factor_spec({**c, "condition_id": "0" * 24}, 1, cctx)


@pytest.mark.parametrize("family,key", [("S04", "exit"), ("S07", "hold")])
def test_d1_adding_an_execution_only_param_to_condition_identity_breaks_the_authority(family, key, monkeypatch):
    keys = {**ss.CONDITION_PARAM_KEYS, family: (*ss.CONDITION_PARAM_KEYS[family], key)}
    exec_keys = {f: tuple(k for k in v if not (f == family and k == key)) for f, v in ss.EXECUTION_ONLY_KEYS.items()}
    monkeypatch.setattr(ss, "CONDITION_PARAM_KEYS", keys)
    monkeypatch.setattr(ss, "EXECUTION_ONLY_KEYS", exec_keys)
    cfgs = ss.build_configs()
    with pytest.raises(ss.GrammarRefusal, match="semantic condition counts"):
        ss.assert_condition_authority(ss.build_conditions(cfgs), cfgs)


def test_d1_no_semantic_coordinate_has_two_factor_ids_and_removal_breaks_the_population_proof(tmp_path, cctx):
    db = tmp_path / "r.sqlite"
    ids = cd.register_all_factors(db, CONDS, cctx)
    seen: dict = {}
    for f in cd.list_factors(db, family=cd.FACTOR_FAMILY):
        seen.setdefault((f["identity"]["params"]["condition_id"], f["identity"]["horizon_periods"]), []).append(f["factor_id"])
    assert len(seen) == 1095 and all(len(v) == 1 for v in seen.values())
    with __import__("contextlib").closing(ResearchResultStore(db)._connect()) as con:  # noqa: SLF001
        con.execute("delete from research_factors where factor_id=?", (ids[-1],))
        con.commit()
    with pytest.raises(RuntimeError, match="missing=1"):
        cd.registered_population_digest(db, ids)


def test_d1_v2_factor_ids_attempts_and_results_cannot_satisfy_the_v3_population(tmp_path, cctx):
    db = tmp_path / "r.sqlite"
    cfgs = ss.build_configs()
    v2 = [cd.v2_factor_spec(c, h, cctx) for c in cfgs for h in ss.CONDITIONAL_HORIZONS]
    v2_ids = [cd.register_factor(db, s) for s in v2]
    assert len(set(v2_ids)) == 2170 and {s.family for s in v2} == {cd.FACTOR_FAMILY_V2}
    st = ResearchResultStore(db)
    for fid in v2_ids[:5]:
        st.begin_factor_evaluation_attempt(factor_id=fid, evaluation_id="e" * 32, origin="v2")
    v3_ids = cd.register_all_factors(db, CONDS, cctx)
    assert not set(v2_ids) & set(v3_ids)
    dig = cd.registered_population_digest(db, v3_ids)
    assert dig["registered"] == 1095 and dig["attempts"] == 0, "V2 attempts must not leak into the V3 population"
    rep = cd.family_fdr_report(db, [])
    assert rep["family"] == cd.FACTOR_FAMILY and sorted(rep["declared_factor_ids"]) == sorted(v3_ids)
    with pytest.raises(RuntimeError, match="missing=1"):
        cd.registered_population_digest(db, v3_ids + ["f" * 32])


def test_d1_conditional_adjacency_uses_only_the_semantic_grid():
    nb = er.conditional_neighbor_map(CONDS)
    for i, c in enumerate(CONDS):
        for j in nb[i]:
            o = CONDS[j]
            assert o["family"] == c["family"]
            diff = {k for k in set(c["params"]) | set(o["params"]) if c["params"].get(k) != o["params"].get(k)}
            assert len(diff) == 1 and diff <= set(ss.CONDITION_PARAM_KEYS[c["family"]])
    leaky = [{**c, "params": {**c["params"], "hold": h}} for c in CONDS if c["family"] == "S07" for h in (1, 3)]
    with pytest.raises(er.RegistryRefusal, match="execution-only"):
        er.conditional_neighbor_map(leaky)


def test_d1_registry_conditional_records_are_keyed_by_semantic_condition(reg_run):
    cond_ids = {c["condition_id"] for c in reg_run["conds"]}
    for e in _edges(reg_run):
        if e["kind"] == "CONDITIONAL_EDGE":
            assert e["condition_id"] in cond_ids and "config_id" not in e and e["source_config_count"] >= 1
    assert all("config_id" not in x and x["condition_id"] in cond_ids for x in _lines(reg_run["out"] / er.OUT_FACTOR_LEDGER))


# ------------------------------------------------------------ factor-level resume / idempotency (CR-02)

import mqk_research.factors.runner as frunner  # noqa: E402

HORIZONS = list(ss.CONDITIONAL_HORIZONS)


class _Env:
    def __init__(self, tmp_path, cctx, cond, U):
        self.db, self.out, self.rec, self.ctx, self.cond, self.U = tmp_path / "f.sqlite", tmp_path / "art", tmp_path / "rec", cctx, cond, U
        self.store = ResearchResultStore(self.db)
        self.fids = {h: cd.register_factor(self.db, cd.factor_spec(cond, h, cctx)) for h in HORIZONS}

    def resolve(self, h, evaluate=None):
        return cd.resolve_factor(self.db, self.out, self.rec, self.U, self.cond, h, self.ctx, origin="t",
                                 cache=cd.PermutationCache(), evaluate=evaluate)

    def attempts(self, h):
        return self.store.list_factor_evaluation_attempts(self.fids[h])

    def statuses(self):
        return {h: [a["status"] for a in self.attempts(h)] for h in HORIZONS}


def _flaky(monkeypatch, fail_on: set):
    """Evaluate wrapper whose infrastructure fault hits INSIDE the registered runner (after the durable attempt opened)."""
    def boom(*a, **k):
        raise RuntimeError("injected infrastructure fault")

    def evaluate(db, out, U, cond, h, ctx, **kw):
        if h in fail_on:
            with monkeypatch.context() as m:
                m.setattr(frunner, "evaluate_factor_ic_ir", boom)
                return cd.evaluate_factor(db, out, U, cond, h, ctx, **kw)
        return cd.evaluate_factor(db, out, U, cond, h, ctx, **kw)
    return evaluate


def _stop_after_fault(env, evaluate):
    """One run that stops at the first infrastructure fault (the process dies there)."""
    done = []
    try:
        for h in HORIZONS:
            done.append((h, env.resolve(h, evaluate)[1]))
    except RuntimeError as exc:
        assert "injected" in str(exc)
    return done


def test_d2_case_a_succeeded_horizon_is_not_rerun_when_a_later_horizon_fails(tmp_path, monkeypatch, U, cctx):
    env = _Env(tmp_path, cctx, _cond("S05", **S05_COND), U)
    assert _stop_after_fault(env, _flaky(monkeypatch, {3})) == [(1, "evaluated")]
    assert env.statuses() == {1: ["succeeded"], 3: ["failed"], 5: [], 10: [], 20: []}
    h1_attempt = env.attempts(1)[0]["attempt_id"]
    resumed = [(h, env.resolve(h)[1]) for h in HORIZONS]
    assert resumed == [(1, "reused"), (3, "retried"), (5, "evaluated"), (10, "evaluated"), (20, "evaluated")]
    assert env.statuses() == {1: ["succeeded"], 3: ["failed", "succeeded"], 5: ["succeeded"], 10: ["succeeded"],
                              20: ["succeeded"]}
    assert env.attempts(1)[0]["attempt_id"] == h1_attempt and len(env.attempts(1)) == 1
    assert env.attempts(3)[1]["evaluation_id"] == env.attempts(3)[0]["evaluation_id"], "retry is the SAME evaluation"


def test_d2_case_b_not_evaluable_horizon_stays_one_terminal_attempt_when_a_later_horizon_fails(tmp_path, monkeypatch, U, cctx):
    env = _Env(tmp_path, cctx, [c for c in CONDS if c["family"] == "S14"][0], U)
    assert _stop_after_fault(env, _flaky(monkeypatch, {3})) == [(1, "evaluated")]
    assert env.statuses()[1] == ["not_evaluable"] and env.statuses()[3] == ["failed"]
    resumed = [(h, env.resolve(h)[1]) for h in HORIZONS]
    assert resumed[0] == (1, "reused") and resumed[1] == (3, "retried")
    assert env.statuses()[1] == ["not_evaluable"], "a terminal NOT_EVALUABLE is never re-attempted"
    assert all(len(env.attempts(h)) == (2 if h == 3 else 1) for h in HORIZONS)


def test_d2_case_c_a_stale_started_attempt_is_never_treated_as_success(tmp_path, U, cctx):
    env = _Env(tmp_path, cctx, _cond("S05", **S05_COND), U)
    aid, _ = env.store.begin_factor_evaluation_attempt(factor_id=env.fids[1], evaluation_id="e" * 32, origin="crashed")
    cd.write_factor_record(env.rec, {"factor_id": env.fids[1], "attempt_id": aid, "evaluation_id": "e" * 32,
                                     "status": "succeeded", "horizon": 1})  # a fabricated success claim
    assert cd.settled_record(env.store, env.rec, env.fids[1]) is None
    rec, action = env.resolve(1)
    assert action == "retried" and rec["attempt_id"] != aid and rec["status"] == "succeeded"
    first, second = env.attempts(1)
    assert (first["status"], first["failure_reason"]) == ("failed", cd.INTERRUPTED_REASON) and second["status"] == "succeeded"


def test_d2_case_d_a_completed_five_horizon_set_resumes_with_zero_new_attempts(tmp_path, U, cctx):
    env = _Env(tmp_path, cctx, _cond("S05", **S05_COND), U)
    first = [env.resolve(h) for h in HORIZONS]
    before = env.statuses()
    again = [env.resolve(h) for h in HORIZONS]
    assert [a for _r, a in again] == ["reused"] * 5 and [r for r, _a in again] == [r for r, _a in first]
    assert env.statuses() == before and all(len(v) == 1 for v in before.values())
    assert cd.pending_conditions(env.db, env.rec, [env.cond], cctx) == []


@pytest.mark.parametrize("family", ["S05", "S14"])
def test_d2_case_e_deleted_result_files_are_rebuilt_without_new_attempts(family, tmp_path, U, cctx):
    cond = _cond("S05", **S05_COND) if family == "S05" else [c for c in CONDS if c["family"] == "S14"][0]
    env = _Env(tmp_path, cctx, cond, U)
    first = [env.resolve(h)[0] for h in HORIZONS]
    import shutil
    shutil.rmtree(env.rec)
    assert cd.pending_conditions(env.db, env.rec, [cond], cctx) == [(0, cond)]
    rebuilt = [env.resolve(h) for h in HORIZONS]
    assert [a for _r, a in rebuilt] == ["reconstructed"] * 5 and [r for r, _a in rebuilt] == first
    assert all(len(env.attempts(h)) == 1 for h in HORIZONS)


def test_d2_unreconstructable_terminal_evidence_fails_closed_instead_of_rerunning(tmp_path, U, cctx):
    env = _Env(tmp_path, cctx, _cond("S05", **S05_COND), U)
    env.resolve(3)
    art = Path(env.attempts(3)[0]["artifact_paths"]["factor_diagnostics"])
    (env.rec / f"{env.fids[3]}.json").unlink()
    saved = art.read_text(encoding="utf-8")
    art.unlink()
    with pytest.raises(cd.FactorResumeRefusal, match="artifact is unavailable"):
        env.resolve(3)
    forged = json.loads(saved)
    forged["input_provenance"]["content_sha256"] = "0" * 64
    art.write_text(json.dumps(forged), encoding="utf-8")
    with pytest.raises(cd.FactorResumeRefusal, match="do not match the registered artifact"):
        env.resolve(3)
    assert len(env.attempts(3)) == 1, "a terminal factor is never re-attempted, even when its evidence is unusable"
    art.write_text(saved, encoding="utf-8")
    assert env.resolve(3)[1] == "reconstructed"


def test_d2_retry_eligibility_is_infrastructure_failure_only_and_conflicts_fail_closed(tmp_path, U, cctx):
    ok = {"status": "failed", "failure_reason": "RuntimeError: boom"}
    assert cd.retry_eligible(ok) and not cd.retry_eligible({"status": "failed", "failure_reason": ""})
    for status in ("succeeded", "not_evaluable"):
        assert not cd.retry_eligible({"status": status, "failure_reason": "zero_variance_factor"})
    env = _Env(tmp_path, cctx, _cond("S05", **S05_COND), U)
    env.resolve(1)
    aid, _ = env.store.begin_factor_evaluation_attempt(factor_id=env.fids[1], evaluation_id=env.attempts(1)[0]["evaluation_id"])
    env.store.finalize_factor_evaluation_attempt(aid, status="failed", expected_factor_id=env.fids[1],
                                                 expected_evaluation_id=env.attempts(1)[0]["evaluation_id"],
                                                 failure_reason="late")
    with pytest.raises(cd.FactorResumeRefusal, match="authority conflict"):
        env.resolve(1)
    other = _cond("S02", sma=50)
    with pytest.raises(cd.FactorResumeRefusal, match="not registered"):
        cd.resolve_factor(env.db, env.out, env.rec, U, other, 5, cctx, origin="t")
    assert len(env.store.list_factors(family=cd.FACTOR_FAMILY)) == 5, "an unfrozen factor must not be auto-registered"


def test_d2_driver_resumes_per_factor_and_registry_outranks_result_files(tmp_path, monkeypatch, U, cctx):
    import run_census as rc
    conds = ss.build_conditions(ss.build_configs())
    c = _cond("S05", **S05_COND)
    ci = conds.index(c)
    env = _Env(tmp_path, cctx, c, U)
    monkeypatch.setattr(rc, "FACTOR_REGISTRY_DB", env.db)
    monkeypatch.setattr(rc, "FACTOR_DIR", tmp_path)
    monkeypatch.setattr(rc, "FACTOR_REC_DIR", env.rec)
    monkeypatch.setattr(rc, "_W", {"U": U, "ctx": cctx, "cache": cd.PermutationCache()})
    env.store.begin_factor_evaluation_attempt(factor_id=env.fids[3], evaluation_id="e" * 32, origin="orphan")
    assert cd.pending_conditions(env.db, env.rec, [c], cctx) == [(0, c)]
    assert rc._eval_condition((ci, c)) == (ci, "ok", {"evaluated": 4, "retried": 1})
    assert env.statuses() == {1: ["succeeded"], 3: ["failed", "succeeded"], 5: ["succeeded"], 10: ["succeeded"], 20: ["succeeded"]}
    recs = cd.load_factor_records(env.db, env.rec, [c], cctx)
    assert [r["horizon"] for r in recs] == HORIZONS
    # a result file with a foreign status / foreign attempt is not authority: the condition is pending again, no rerun
    p = cd.factor_record_path(env.rec, env.fids[5])
    good = p.read_text(encoding="utf-8")
    p.write_text(good.replace('"status":"succeeded"', '"status":"failed"', 1), encoding="utf-8")
    assert cd.pending_conditions(env.db, env.rec, [c], cctx) == [(0, c)]
    with pytest.raises(cd.FactorResumeRefusal, match="not settled"):
        cd.load_factor_records(env.db, env.rec, [c], cctx)
    assert rc._eval_condition((ci, c)) == (ci, "ok", {"reused": 4, "reconstructed": 1})
    assert cd.read_factor_record(env.rec, env.fids[5]) == json.loads(good)
    assert all(len(env.attempts(h)) == (2 if h == 3 else 1) for h in HORIZONS)


# ----------------------------------------------------------------- V3 factor-only freeze gate (D4)

FAKE_BINDING = {"strategy_population_root": "r" * 64, "strategy_cell_count": 7, "manifest_order_root": "m" * 64,
                "strategy_attempts": 7, "strategy_attempts_succeeded": 7, "strategy_attempts_failed": 0,
                "strategy_chunks_root_sha256": "c" * 64, "strategy_search_ledger_sha256": "l" * 64}


@pytest.fixture(scope="module")
def frozen_v3(tmp_path_factory, cctx):
    db = tmp_path_factory.mktemp("v3") / "f.sqlite"
    gid = ss.build_condition_grammar(ss.build_configs())["condition_grammar_id"]
    freeze = cd.factor_freeze_record(CONDS, cctx, gid, FAKE_BINDING)
    ids = cd.register_factor_population(db, CONDS, cctx, freeze)
    return db, freeze, ids, gid


def _copy_db(frozen_v3, tmp_path):
    import shutil
    dst = tmp_path / "copy.sqlite"
    shutil.copy(frozen_v3[0], dst)
    return dst


def test_d4_factor_freeze_registers_all_1095_with_zero_attempts_and_binds_the_strategy_state(frozen_v3, cctx):
    db, freeze, ids, _gid = frozen_v3
    r = cd.require_frozen_factor_population(db, CONDS, cctx, freeze, allow_attempts=False)
    assert r["registered"] == len(ids) == 1095 and r["attempts"] == 0
    assert freeze["condition_count"] == 219 and freeze["conditional_horizons"] == HORIZONS
    assert freeze["strategy_binding"] == FAKE_BINDING and freeze["v2_conditional_disposition"] == ss.V2_CONDITIONAL_DISPOSITION
    assert freeze["conditional_factor_population_root"] == ss.sha256_canonical(sorted(ids))


def test_d4_gate_refuses_attempts_a_different_marker_a_changed_population_and_partial_registries(frozen_v3, cctx, tmp_path):
    db, freeze, ids, gid = frozen_v3
    work = _copy_db(frozen_v3, tmp_path)
    other = {**freeze, "strategy_binding": {**FAKE_BINDING, "strategy_search_ledger_sha256": "x" * 64}}
    with pytest.raises(cd.FactorFreezeRefusal, match="marker absent or different"):
        cd.require_frozen_factor_population(work, CONDS, cctx, other, allow_attempts=True)
    with pytest.raises(cd.FactorFreezeRefusal, match="does not describe"):
        cd.require_frozen_factor_population(work, CONDS[:-1], cctx, freeze, allow_attempts=True)
    with pytest.raises(cd.FactorFreezeRefusal, match="extra=5"):
        cd.require_frozen_factor_population(work, CONDS[:-1], cctx, cd.factor_freeze_record(CONDS[:-1], cctx, gid, FAKE_BINDING),
                                            allow_attempts=True)
    ResearchResultStore(work).begin_factor_evaluation_attempt(factor_id=ids[0], evaluation_id="e" * 32, origin="x")
    with pytest.raises(cd.FactorFreezeRefusal, match="attempts already exist"):
        cd.require_frozen_factor_population(work, CONDS, cctx, freeze, allow_attempts=False)
    assert cd.require_frozen_factor_population(work, CONDS, cctx, freeze, allow_attempts=True)["attempts"] == 1
    with __import__("contextlib").closing(ResearchResultStore(work)._connect()) as con:  # noqa: SLF001
        con.execute("delete from research_factors where factor_id=?", (ids[-1],))
        con.commit()
    with pytest.raises(cd.FactorFreezeRefusal, match="missing=1"):
        cd.require_frozen_factor_population(work, CONDS, cctx, freeze, allow_attempts=True)


def test_d4_a_v2_only_registry_cannot_satisfy_the_v3_freeze(tmp_path, cctx, frozen_v3):
    db = tmp_path / "v2.sqlite"
    for c in ss.build_configs():
        for h in HORIZONS:
            cd.register_factor(db, cd.v2_factor_spec(c, h, cctx))
    with pytest.raises(cd.FactorFreezeRefusal, match="missing=1095"):
        cd.require_frozen_factor_population(db, CONDS, cctx, frozen_v3[1], allow_attempts=True)


def test_d4_strategy_binding_requires_the_accepted_terminal_state_chunks_and_ledger(tmp_path, mini, reference_run):
    root, st = reference_run
    ledger = tmp_path / "ledger.jsonl"
    ledger.write_text("accepted\n", encoding="utf-8")
    good = cs.sha256_file(ledger)
    kw = dict(chunk_size=13, ledger_path=ledger, expected_ledger_sha256=good)
    b = cs.strategy_binding(st, mini["space"], mini["cells"], root, **kw)
    n = len(mini["cells"])
    assert b["strategy_attempts"] == b["strategy_attempts_succeeded"] == b["strategy_cell_count"] == n
    assert b["strategy_search_ledger_sha256"] == good and len(b["strategy_chunks_root_sha256"]) == 64
    with pytest.raises(cs.GateRefusal, match="search ledger differs"):
        cs.strategy_binding(st, mini["space"], mini["cells"], root, **{**kw, "expected_ledger_sha256": "0" * 64})
    import shutil
    work = tmp_path / "w"
    shutil.copytree(root / "chunks", work / "chunks")
    p0 = cs.chunk_path(work, 0)
    p0.write_text(p0.read_text(encoding="utf-8").replace("EVALUABLE", "EVALUABLF", 1), encoding="utf-8")
    assert cs.strategy_binding(st, mini["space"], mini["cells"], work, **kw)["strategy_chunks_root_sha256"] != b["strategy_chunks_root_sha256"]
    p0.unlink()
    with pytest.raises(cs.GateRefusal, match="missing StrategyEdge chunk"):
        cs.strategy_binding(st, mini["space"], mini["cells"], work, **kw)
    (tmp_path / "fresh").mkdir()
    fresh = _fresh_store(tmp_path / "fresh", mini)
    with pytest.raises(cs.GateRefusal, match="not in the accepted terminal state"):
        cs.strategy_binding(fresh, mini["space"], mini["cells"], root, **kw)


def test_d4_committed_factor_freeze_proof_binds_manifests_population_and_the_accepted_strategy_state():
    proof, disp = _load("FACTOR_FREEZE_PROOF_V3.json"), _load("CONDITIONAL_V2_DISPOSITION.json")
    ev2, space, grammar = _load("CAMPAIGN_EVIDENCE_V2.json"), _load("ALPHA_CENSUS_SEARCH_SPACE_V2.json"), _load("ALPHA_CENSUS_GRAMMAR_V2.json")
    assert proof["scope"] == "FACTOR_ONLY" and proof["strategy_attempts_created_by_this_mission"] == 0
    assert proof["semantic_condition_count"] == 219 and proof["registered_factors"] == proof["conditional_factor_count"] == 1095
    assert proof["factor_evaluation_attempts"] == 0 and proof["conditional_horizons"] == HORIZONS
    assert proof["per_family_condition_counts"] == ss.EXPECTED_CONDITION_FAMILY_COUNTS
    b = proof["strategy_binding"]
    assert b["strategy_cell_count"] == b["strategy_attempts"] == b["strategy_attempts_succeeded"] == space["strategy_trial_count"] == 38192
    assert b["strategy_attempts_failed"] == 0 and proof["strategy_edge_config_count"] == len(grammar["configs"]) == 434
    assert b["strategy_search_ledger_sha256"] == ev2["output_sha256"]["search_ledger_v2.jsonl"]
    assert proof["strategy_edges_accepted"] == {"DISCOVERED_WEAK": 2851, "DISCOVERED_MODERATE": 789, "DISCOVERED_STRONG": 0}
    cg = ss.build_condition_grammar(ss.build_configs())
    assert _load("ALPHA_CENSUS_CONDITION_GRAMMAR_V3.json") == cg and proof["condition_grammar_id"] == cg["condition_grammar_id"]
    assert proof["condition_source_mapping_sha256"] == cg["source_mapping_sha256"]
    for name, digest in proof["manifest_file_sha256"].items():
        assert _norm_sha(EXP / name) == digest, name
    eq = proof["condition_equivalence_real_data"]
    assert eq["conditions"] == 219 and eq["mismatches"] == 0 and eq["multi_source_conditions"] == 150
    assert eq["source_config_series_compared"] == 434 * eq["symbols"] == 38192
    uni = _load("ALPHA_CENSUS_UNIVERSE_V2.json")
    ctx = cd.population_context(uni, ss.build_protocol(), ss.build_partitions(), _load("ALPHA_CENSUS_BARS_MANIFEST_V2.json"))
    v3 = cd.expected_factor_ids(CONDS, ctx)
    assert ss.sha256_canonical(sorted(v3)) == proof["conditional_factor_population_root"]
    v2 = sorted(cd.v2_factor_spec(c, h, ctx).compute_factor_id() for c in ss.build_configs() for h in HORIZONS)
    assert disp["disposition"] == proof["v2_conditional_disposition"] == "REJECTED_SEMANTIC_DUPLICATE_FACTOR_POPULATION"
    assert disp["v2_factor_count"] == len(v2) == 2170 and disp["v2_population_root"] == ss.sha256_canonical(v2)
    assert not set(v2) & set(v3) and disp["authoritative"] is False and disp["v2_results_used_to_choose_v3_parameters"] is False
    assert pd.Timestamp(proof["max_economic_input_end_ts"]) <= pd.Timestamp("2023-12-29T23:59:59", tz="UTC")
