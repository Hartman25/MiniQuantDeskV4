"""Census-02 result-independent campaign: approved policy, frozen populations, Strategy qualification, factor evaluator,
registration/resume, and the freeze-FIRST call-order proof. Synthetic bars only: no provider, network, cache or real bar."""

from __future__ import annotations

import builtins
import copy
import io
import json
import socket
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c2_testing as ct  # noqa: E402  (also puts the Census-02 modules on sys.path)

import c2_borrow as bw  # noqa: E402
import c2_data  # noqa: E402
import c2_factor_eval as fe  # noqa: E402
import c2_factors as fx  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_policy as pol  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_runner as rn  # noqa: E402
import c2_signals as sg2  # noqa: E402
import c2_simulate as sim2  # noqa: E402
import c2_strategy as st  # noqa: E402
import conditional as cd1  # noqa: E402
import data as dt1  # noqa: E402
import edge_registry as er  # noqa: E402
import search_space as ss1  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

EXP2 = ct.EXP2
DECISIONS = pol.approved_decisions()
PID = "p" * 32


# ============================================================================================== approved policy (P1-P8)
def test_operator_policy_is_encoded_exactly():
    d = pol.approved_decisions()
    assert d["borrow_policy"] == "EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION"
    assert d["etf_borrow_assumption"] == {
        "etf_short_scope": ["DIA", "EEM", "EFA", "GLD", "IEF", "IWM", "QQQ", "SLV", "SPY", "TLT", "VTI", "XLB", "XLE", "XLF",
                            "XLI", "XLK", "XLP", "XLU", "XLV", "XLY"],
        "annual_borrow_fee_bps": 100.0, "availability": "ALWAYS_AVAILABLE_FOR_SCOPE_ASSUMED", "recall": "NONE_ASSUMED",
        "short_rebate": "ZERO"}
    assert (d["grammar_tiers"], d["complement_handling"]) == ("H+L", "REGISTER_ALL_TAG_COMPLEMENTS")
    assert d["benchmark_rule"] == "SIDE_AWARE_SHORT_NET_AND_PASSIVE_SHORT_ALPHA_LONGSHORT_NET_VS_CASH"
    assert (d["multiple_testing_denominator"], d["conditional_scope"], d["ssr_handling"]) == (
        "LOCAL_WITH_GLOBAL_DISCLOSURE", "ALL_SEED_SYMBOLS", "FLAG_ONLY")
    assert d["funnel_thresholds"] == {
        "trade_count_confidence_band": {"kind": "CLASSIFICATION_WITH_TYPED_MINIMUM", "under_5": "INSUFFICIENT",
                                        "5_to_14": "LOW_SAMPLE", "15_to_29": "MODERATE_SAMPLE", "30_plus": "STRONG_SAMPLE",
                                        "hard_gate": "closed_round_trips >= 5 only"},
        "year_stability": "REPORT_ONLY_NO_GATE", "regime_concentration": "REPORT_ONLY_NO_GATE",
        "parameter_neighborhood": "REPORT_ONLY_NO_GATE",
        "portfolio_mdd_worst5day": "DEFER_TO_PORTFOLIO_RISK_SUITABILITY_STAGE_USING_MAIN_RISK_BAR"}
    assert pr.validate_decisions(d) and pol.GLOBAL_DISCLOSURE["census01_strategy_trials"] == 38_192
    assert pol.GLOBAL_DISCLOSURE["census01_conditional_factors"] == 1_095
    assert set(d["etf_borrow_assumption"]["etf_short_scope"]) <= bw.seed_universe_symbols()   # explicit list, inside the seed


def test_policy_artifact_matches_code_and_holds_no_result():
    art = json.loads((EXP2 / "CENSUS02_OPERATOR_POLICY.json").read_text(encoding="utf-8"))
    assert art == pol.policy_document()
    assert art["approval"]["approved_before_result_1"] is True and art["approval"]["contains_results"] is False
    assert art["approval"]["status"] == "APPROVED_BY_OPERATOR"
    assert "borrow truth" in art["approval"]["annual_borrow_fee_bps_meaning"]
    assert not _result_keys(art)
    assert "CENSUS02_OPERATOR_POLICY.json" in " ".join(pr.AUTHORITY_DATA)                   # bound as authority data


def _result_keys(obj, found=None):
    found = set() if found is None else found
    keys = {"net_pnl_usd", "net_alpha_usd", "p_value", "q_values", "mean_ic", "event_count", "direction_adjusted_effect"}
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k in keys and isinstance(v, (int, float, list, dict)) and not isinstance(v, bool):
                found.add(k)
            _result_keys(v, found)
    elif isinstance(obj, list):
        for v in obj:
            _result_keys(v, found)
    return found


def test_conditional_statistics_and_request_contract_are_the_accepted_census01_values():
    s = pr.CONDITIONAL_STATISTICS
    assert (s["empirical_null"]["n_permutations"], s["empirical_null"]["base_seed"]) == (ss1.N_PERMUTATIONS, ss1.BASE_SEED) == (200, 0)
    assert (s["fdr"]["alpha"], s["conditional_p_alpha"], s["min_events"]) == (ss1.DISCOVERY_FDR_ALPHA, ss1.CONDITIONAL_P_ALPHA,
                                                                              ss1.MIN_CONDITIONAL_EVENTS) == (0.1, 0.1, 30)
    assert (s["n_quantiles"], s["min_cross_section"], s["min_periods"]) == (cd1.N_QUANTILES, cd1.MIN_CROSS_SECTION, cd1.MIN_PERIODS)
    assert (s["window_start_utc"], s["window_end_utc"], s["label_protocol_version"]) == (
        cd1.WINDOW_START_UTC, cd1.WINDOW_END_UTC, cd1.LABEL_PROTOCOL_VERSION)
    assert {k: pr.DATA_REQUEST_CONTRACT[k] for k in dt1.REQUEST_CONTRACT} == dt1.REQUEST_CONTRACT   # Census-01 contract, unchanged
    assert pr.DATA_REQUEST_CONTRACT["feed"] == "sip" and pr.DATA_REQUEST_CONTRACT["adjustment"] == "all"
    assert pr.DATA_REQUEST_CONTRACT["end_utc_exclusive"].startswith("2024-01-01")
    assert "multiple_testing" in s and "LOCAL_WITH_GLOBAL_DISCLOSURE" in s["multiple_testing"]
    # the structural protocol (hence protocol_id) carries them
    st_ = pr.build_structural_protocol()
    assert st_["conditional_statistics"] == s and st_["data_request_contract"] == pr.DATA_REQUEST_CONTRACT
    assert st_["global_disclosure"] == pol.GLOBAL_DISCLOSURE


def test_direct_data_entrance_without_the_freeze_gate_is_refused_before_any_io(monkeypatch):
    """Even a caller that skips the runner cannot acquire or read: the latch is set only by a passed require_freeze."""
    for fn in ("acquire_symbol", "acquire_universe", "load_symbol_bars", "classify_eligibility", "load_alpaca_env"):
        monkeypatch.setattr(dt1, fn, lambda *a, **k: pytest.fail("acquisition reached without the freeze gate"))
    for fn in ("build_bars_manifest", "load_bars"):
        monkeypatch.setattr(c2_data.ce, fn, lambda *a, **k: pytest.fail("bars read without the freeze gate"))
    pr._GATE.clear()
    with pytest.raises(pr.FreezeRefusal, match="require_freeze has not passed"):
        c2_data.load_discovery_universe(Path("unused"), pr.DATA_REQUEST_CONTRACT, PID)
    pr._GATE.update({"protocol_id": "someone-else"})                                    # a different protocol's gate does not count
    with pytest.raises(pr.FreezeRefusal):
        c2_data.load_discovery_universe(Path("unused"), pr.DATA_REQUEST_CONTRACT, PID)
    pr._GATE.clear()
    pr._GATE.update({"protocol_id": PID})                                               # a stale pass from an earlier check
    c2_data_calls = []
    monkeypatch.setattr(dt1, "acquire_universe", lambda *a, **k: c2_data_calls.append(1))
    with pytest.raises(pr.FreezeRefusal):                                               # a failed gate also CLOSES the latch
        pr.require_freeze(pr.REPO, pr.HERE / "NO_SUCH_FREEZE.json")
    assert pr._GATE == {}
    with pytest.raises(pr.FreezeRefusal):
        c2_data.load_discovery_universe(Path("unused"), pr.DATA_REQUEST_CONTRACT, PID)
    assert c2_data_calls == []


def test_data_contract_mismatch_is_refused_before_any_acquisition(monkeypatch):
    pr._GATE.clear()
    pr._GATE.update({"protocol_id": PID})
    c2_data.require_request_contract(copy.deepcopy(pr.DATA_REQUEST_CONTRACT))
    for key, val in (("feed", "iex"), ("provider", "other"), ("adjustment", "raw"), ("asof", "2026-01-01"),
                     ("end_utc_exclusive", "2025-01-01T00:00:00+00:00")):
        with pytest.raises(c2_data.DataContractRefusal):
            c2_data.require_request_contract({**pr.DATA_REQUEST_CONTRACT, key: val})
    monkeypatch.setattr(dt1, "acquire_universe", lambda *a, **k: pytest.fail("acquisition reached with a bad contract"))
    with pytest.raises(c2_data.DataContractRefusal):
        c2_data.load_discovery_universe(Path("unused"), {**pr.DATA_REQUEST_CONTRACT, "feed": "iex"}, PID)
    pr._GATE.clear()


# ============================================================================================== frozen populations
def test_strategy_population_is_exactly_470_by_20_and_result_independent():
    cells = pop.strategy_cells(DECISIONS, PID)
    scope = set(DECISIONS["etf_borrow_assumption"]["etf_short_scope"])
    assert (len(cells), len(scope), len({c[3] for c in cells})) == (9400, 20, 9400)
    assert {c[2] for c in cells} == scope and len({c[1]["config_id"] for c in cells}) == 470
    assert all(not c[2].startswith("A") for c in cells) and "AAPL" not in {c[2] for c in cells}      # no equity cell exists
    a = pop.strategy_population(DECISIONS, PID)
    assert a["trial_count"] == 9400 and a["config_count"] == 470 and a["class_c_symbol_count"] == 20
    assert a == pop.strategy_population(DECISIONS, PID)                                              # deterministic
    assert a["population_root"] != pop.strategy_population(DECISIONS, "q" * 32)["population_root"]  # protocol id is identity
    assert pop.sorted_root(c[3] for c in cells) == a["population_root"]
    # identity carries no result: the identity key set is closed and polluted configs map to the same id
    cfg = {**cells[0][1], "m": {"net_pnl_usd": 1.0}, "outcome": "QUALIFIED", "attempt_id": "x:att0003"}
    assert gr.trial_id(gr.trial_identity(cfg, cells[0][2], pop.strategy_ids(DECISIONS, PID))) == cells[0][3]


def test_complement_tags_survive_and_are_counted_not_dropped():
    a = pop.strategy_population(DECISIONS, PID)
    assert (a["complement_tagged_configs"], a["complement_tagged_trials"]) == (56, 56 * 20)
    tagged = {c["family"] for c in gr.build_configs() if gr.tags(c)["complement_of_census01"]}
    assert tagged == set(gr.COMPLEMENT_FAMILIES)
    assert all(gr.tags(c)["mirror_of"] for c in gr.build_configs() if c["side"] == "short")
    assert all(len(gr.tags(c)["legs"]) == 2 for c in gr.build_configs() if c["side"] == "long_short")
    exclusion = {**DECISIONS, "complement_handling": "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE"}
    assert pop.strategy_population(exclusion, PID)["trial_count"] == 8280 and pop.factor_population(exclusion)["factor_count"] == 935


def test_factor_population_is_215_by_5_lower_is_better_over_all_88_seed_symbols():
    f = pop.factor_population(DECISIONS)
    assert (f["condition_count"], f["factor_count"], f["direction"], f["scope"], f["scope_symbol_count"]) == (
        215, 1075, "lower_is_better", "ALL_SEED_SYMBOLS", 88)
    assert f["horizons"] == [1, 3, 5, 10, 20] and f == pop.factor_population(DECISIONS)
    assert len(pop.seed_symbols()) == 88 and "AAPL" in pop.seed_symbols()               # equities participate in label evidence
    assert len(pop.factor_coordinates(DECISIONS)) == 1075                               # one factor per condition x horizon, NOT x88


def test_population_authority_is_what_the_freeze_document_records():
    auth = pop.population_authority(DECISIONS, PID)
    assert set(auth) == {"strategy_population", "factor_population"}
    assert auth["strategy_population"]["trial_count"] == 9400 and auth["factor_population"]["factor_count"] == 1075


# ======================================================================================= Strategy qualification (P4/P8)
def _rec(net, rt, short_alpha=None, **extra):
    m = {"cash_zero": {"net_pnl_usd": net, "round_trips": rt, **extra}}
    if short_alpha is not None:
        m["passive_short_hold"] = {"net_alpha_usd": short_alpha}
    return {"d": "EVALUABLE", "executable_pnl": True, "m": m}


RULE = DECISIONS["benchmark_rule"]


@pytest.mark.parametrize("rt,band", [(0, "INSUFFICIENT"), (4, "INSUFFICIENT"), (5, "LOW_SAMPLE"), (14, "LOW_SAMPLE"),
                                      (15, "MODERATE_SAMPLE"), (29, "MODERATE_SAMPLE"), (30, "STRONG_SAMPLE"), (500, "STRONG_SAMPLE")])
def test_trade_count_band_boundaries(rt, band):
    assert st.trade_band(rt) == band


@pytest.mark.parametrize("rt", [5, 6, 14, 15, 29])
def test_no_hard_30_trade_veto_only_the_typed_minimum_of_5(rt):
    outcome, band = st.strategy_outcome("short", _rec(10.0, rt, 3.0), RULE)
    assert outcome == "QUALIFIED" and band == st.trade_band(rt) != "STRONG_SAMPLE"          # Pass-2's blanket >=30 veto is gone
    assert st.strategy_outcome("long_short", _rec(10.0, rt), RULE)[0] == "QUALIFIED"


@pytest.mark.parametrize("rt", [0, 3, 4])
def test_fewer_than_5_closed_round_trips_is_typed_insufficient(rt):
    for side, rec in (("short", _rec(10.0, rt, 3.0)), ("long_short", _rec(10.0, rt))):
        assert st.strategy_outcome(side, rec, RULE) == ("INSUFFICIENT_CLOSED_ROUND_TRIPS", "INSUFFICIENT")


@pytest.mark.parametrize("side,net,alpha,expect", [
    ("short", 5.0, 3.0, "QUALIFIED"), ("short", 5.0, 0.0, "NOT_QUALIFIED"), ("short", 5.0, -1.0, "NOT_QUALIFIED"),
    ("short", 0.0, 9.0, "NOT_QUALIFIED"), ("short", -2.0, 9.0, "NOT_QUALIFIED"),
    ("long_short", 5.0, None, "QUALIFIED"), ("long_short", 0.0, None, "NOT_QUALIFIED"), ("long_short", -1.0, None, "NOT_QUALIFIED")])
def test_side_aware_qualification_is_exact(side, net, alpha, expect):
    assert st.strategy_outcome(side, _rec(net, 10, alpha), RULE)[0] == expect


def test_report_only_items_never_veto():
    rec = _rec(10.0, 8, 2.0, year_concentration_share=1.0, regime_concentration_share=1.0, max_drawdown_usd=1e9,
               max_drawdown_frac_of_budget=1e6, trade_count=8)
    assert st.strategy_outcome("short", rec, RULE)[0] == "QUALIFIED"
    rows = [{"t": f"t{i}", "c": i, "s": "SPY", "outcome": o} for i, o in enumerate(["QUALIFIED", "NOT_QUALIFIED", "QUALIFIED"])]
    cfgs = [{"family": "SH02", "params": {"sma": v}} for v in (20, 50, 100)]
    rep = st.neighborhood_report(rows, cfgs)                      # report-only: describes, never removes a cell
    assert set(rep) == {"t0", "t1", "t2"} and rep["t1"]["qualified_neighbor_share"] == 1.0 and rep["t0"]["neighbors"] == 1


def test_non_finite_metrics_are_non_evaluable_not_qualified():
    for bad in (float("nan"), float("inf"), None):
        assert st.strategy_outcome("short", _rec(bad, 10, 3.0), RULE)[0] == "NON_EVALUABLE"
        assert st.strategy_outcome("short", _rec(5.0, 10, bad), RULE)[0] == "NON_EVALUABLE"
    assert st.strategy_outcome("short", {"d": "NON_EVALUABLE_X", "m": None}, RULE)[0] == "NON_EVALUABLE"


@pytest.fixture(scope="module")
def mini_universe():
    syms = ["SPY", "DIA", "QQQ", "TSLA"]
    return fe.Universe2({s: ct.noise_bars(s, i + 3, 700, gap=0.02) for i, s in enumerate(syms)})


def test_class_a_equity_is_hypothesis_only_and_never_reaches_the_simulator(mini_universe, monkeypatch):
    cfg = next(c for c in gr.build_configs() if c["family"] == "SH02")
    monkeypatch.setattr(sim2, "simulate_signed", lambda *a, **k: pytest.fail("simulator reached for a Class-A equity"))
    rec = st.evaluate_cell(mini_universe.sd["TSLA"], cfg, "TSLA", DECISIONS)
    assert rec["outcome"] == "NOT_EXECUTABLE_HYPOTHESIS_ONLY" and rec["m"] is None and rec["executable_pnl"] is False
    assert rec["evidence_class"] == bw.EVIDENCE_A


def test_class_c_cell_is_executable_and_carries_tags_ssr_and_report_only_fields(mini_universe):
    cfg = next(c for c in gr.build_configs() if c["family"] == "LS01" and c["params"] == {"lookback": 63, "cadence": "daily"})
    rec = st.evaluate_cell(mini_universe.sd["DIA"], cfg, "DIA", DECISIONS)
    assert rec["executable_pnl"] is True and rec["evidence_class"] == bw.EVIDENCE_C and rec["outcome"] in st.OUTCOMES
    assert rec["tags"]["complement_of_census01"] is True and rec["tags"]["legs"] == ["S01", "SH01"]
    assert rec["ssr"]["basis"] == "daily_bar_rule201_possible_hazard_flag_only_intraday_sequence_unknown" and rec["ssr"]["short_entries"] >= 0
    assert set(rec["m"]) == {"cash_zero", "passive_long_hold", "passive_short_hold"}
    assert rec["report_only"]["year_concentration_share"] is not None or rec["report_only"]["round_trips"] == 0


def test_ssr_is_flag_only_a_hazard_never_rejects_or_defers_a_fill(monkeypatch):
    df = ct.noise_bars("SSR", 5, 300)
    df.loc[100, "low"] = round(float(df.loc[99, "close"]) * 0.80, 2)                       # >=10% below the prior close
    sd = sg2.build_symbol_data("SSR", df)
    sd.regime = np.ones(sd.n, np.int8)
    d = np.zeros(sd.n, np.int8)
    d[99:104] = -1                                                                         # short decided after bar 99 -> fills bar 100
    monkeypatch.setattr(sg2, "build", lambda sd_, fam, params: sg2.SigS(d, np.zeros(sd_.n, bool), 60))
    cfg = {"family": "SH02", "side": "short", "params": {"sma": 20}}
    rec = sim2.evaluate_short_cell(sd, cfg, bw.EVIDENCE_C, borrow_fee_bps_annual=100.0)
    assert rec["ssr"] == {"basis": "daily_bar_rule201_possible_hazard_flag_only_intraday_sequence_unknown", "short_entries": 1, "ssr_hazard_entries": 1}
    direct = sim2.simulate_signed(sd.hm, sd.lm, sd.cm, d, 60, borrow_fee_bps_annual=100.0)
    assert list(direct.entries) == [100]                                                   # the flagged fill still happens
    assert rec["m"]["cash_zero"]["net_pnl_usd"] == pytest.approx(float(direct.net.sum()), abs=1e-9)
    assert rec["m"]["cash_zero"]["trade_count"] == 1


def test_ineligible_class_c_symbol_is_a_typed_row_not_a_dropped_trial(mini_universe):
    cell = (0, gr.build_configs()[0], "DIA", "ac02-x")
    row = rn.evaluate_cell_row(mini_universe, cell, DECISIONS, {"DIA": {"disposition": "EXCLUDED_INSUFFICIENT_HISTORY"}})
    assert row["t"] == "ac02-x" and row["d"] == "NON_EVALUABLE_EXCLUDED_INSUFFICIENT_HISTORY" and row["outcome"] == "NON_EVALUABLE"
    assert row["m"] is None and row["executable_pnl"] is False
    missing = rn.evaluate_cell_row(mini_universe, cell, DECISIONS, {})
    assert missing["outcome"] == "NON_EVALUABLE"


# ==================================================================================== factor evaluator (D4/P5/P6)
FSYMS = ["SPY"] + [f"S{i:02d}" for i in range(11)]
SH11_COND = next(c for c in pop.conditions(DECISIONS) if c["family"] == "SH11" and c["params"] == {"up_sessions": 2, "trend": "none"})


def _mini_ctx(tag):
    return fe.population_context({"symbols": FSYMS, "symbol_count": len(FSYMS),
                                  "survivorship_classification": "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME"},
                                 {"manifest_sha256": tag}, PID)


@pytest.fixture(scope="module")
def factor_runs(tmp_path_factory):
    root = tmp_path_factory.mktemp("factors")
    db, out, rec_dir = root / "f.sqlite", root / "eval", root / "rec"
    results, items = {}, []
    for tag, push in (("pos", -0.03), ("neg", 0.03)):
        U = fe.Universe2({s: ct.streak_reversal_bars(s, 100 + i, 700, push) for i, s in enumerate(FSYMS)})
        spec = fx.factor_spec(SH11_COND, 1, _mini_ctx(tag))
        items.append((SH11_COND, 1, spec))
        results[tag] = {"U": U, "spec": spec}
    fe.register_factor_population(db, items)
    for tag in ("pos", "neg"):
        rec, action = fe.resolve_factor(db, out, rec_dir, results[tag]["U"], SH11_COND, 1, results[tag]["spec"])
        results[tag].update(rec=rec, action=action)
    return {"db": db, "out": out, "rec_dir": rec_dir, "items": items, **results}


def test_short_hypothesis_effect_sign_is_minus_raw_and_labels_are_never_pnl(factor_runs):
    pos, neg = factor_runs["pos"]["rec"], factor_runs["neg"]["rec"]
    for rec in (pos, neg):
        assert rec["status"] == "succeeded" and rec["direction"] == "lower_is_better" and rec["executable_pnl"] is False
    ep, en = pos["events"], neg["events"]
    assert ep["event_count"] >= 30 and ep["effect"] < 0 < ep["direction_adjusted_effect"]            # falls after the event: favorable
    assert en["effect"] > 0 > en["direction_adjusted_effect"]                                          # rises after: unfavorable
    assert ep["direction_adjusted_effect"] == pytest.approx(-ep["effect"])
    assert 0 < pos["pvalue"]["p_value"] <= 1 and pos["pvalue"]["n_permutations_used"] == 200 and pos["pvalue"]["base_seed"] == 0
    assert factor_runs["pos"]["action"] == "evaluated"


def test_label_is_the_symbol_demeaned_forward_return_not_a_position_pnl(factor_runs):
    frame, aux = fe.build_frame(factor_runs["pos"]["U"], SH11_COND, 1)
    assert set(frame["factor_value"].unique()) <= {0.0, 1.0}
    per_symbol_mean = frame.groupby("symbol")["label_fwd_ret"].mean()
    assert float(per_symbol_mean.abs().max()) < 1e-12                                  # unconditional same-symbol mean removed
    assert (pd_ts := frame["label_end_ts_utc"] > frame["period_ts_utc"]).all() and pd_ts.all()
    assert frame["label_end_ts_utc"].max() <= "2023-12-31T23:59:59+00:00"              # labels never reach the discovery fence
    assert len(aux) == len(frame)


def test_conditional_classification_uses_the_accepted_rule_and_direction(factor_runs):
    fdr = {"status": "incomplete"}
    assert er.conditional_class(factor_runs["pos"]["rec"], fdr) in ("DISCOVERED_WEAK", "DISCOVERED_MODERATE")
    assert er.conditional_class(factor_runs["neg"]["rec"], fdr) is None                              # wrong-signed evidence never classes


def test_terminal_factor_is_reused_never_reattempted_and_creates_no_new_factor(factor_runs):
    db, out, rec_dir = factor_runs["db"], factor_runs["out"], factor_runs["rec_dir"]
    spec = factor_runs["pos"]["spec"]
    fid = spec.compute_factor_id()
    store = ResearchResultStore(db)
    before = (len(store.list_factors(family=fx.FACTOR_FAMILY)), len(store.list_factor_evaluation_attempts(fid)))
    rec, action = fe.resolve_factor(db, out, rec_dir, factor_runs["pos"]["U"], SH11_COND, 1, spec,
                                    evaluate=lambda *a, **k: pytest.fail("terminal factor re-attempted"))
    assert action == "reused" and rec == factor_runs["pos"]["rec"]
    assert (len(store.list_factors(family=fx.FACTOR_FAMILY)), len(store.list_factor_evaluation_attempts(fid))) == before == (2, 1)


def test_interrupted_attempt_retries_as_a_new_attempt_of_the_same_factor(tmp_path):
    db = tmp_path / "f.sqlite"
    spec = fx.factor_spec(SH11_COND, 1, _mini_ctx("retry"))
    fe.register_factor_population(db, [(SH11_COND, 1, spec)])
    fid = spec.compute_factor_id()
    store = ResearchResultStore(db)
    store.begin_factor_evaluation_attempt(factor_id=fid, evaluation_id=fe.expected_evaluation_id(spec), origin="crashed")

    def evaluate(*_a, **_k):                                     # what the real evaluator does first: open a NEW attempt
        aid, idx = store.begin_factor_evaluation_attempt(factor_id=fid, evaluation_id=fe.expected_evaluation_id(spec), origin="retry")
        return {"factor_id": fid, "attempt_id": aid, "attempt_index": idx}

    rec, action = fe.resolve_factor(db, tmp_path / "o", tmp_path / "r", None, SH11_COND, 1, spec, evaluate=evaluate)
    attempts = store.list_factor_evaluation_attempts(fid)
    assert action == "retried" and [(a["attempt_index"], a["status"]) for a in attempts] == [(1, "failed"), (2, "started")]
    assert attempts[0]["failure_reason"] == fe.INTERRUPTED_REASON
    assert len(store.list_factors(family=fx.FACTOR_FAMILY)) == 1 and rec["attempt_index"] == 2   # same factor, new attempt


def test_foreign_or_unclassified_failed_attempts_are_refused_not_retried(tmp_path):
    db = tmp_path / "f.sqlite"
    spec = fx.factor_spec(SH11_COND, 1, _mini_ctx("foreign"))
    fe.register_factor_population(db, [(SH11_COND, 1, spec)])
    fid, store = spec.compute_factor_id(), ResearchResultStore(db)
    store.begin_factor_evaluation_attempt(factor_id=fid, evaluation_id="e" * 32, origin="foreign")
    with pytest.raises(fe.FactorResumeRefusal):
        fe.resolve_factor(db, tmp_path / "o", tmp_path / "r", None, SH11_COND, 1, spec, evaluate=lambda *a, **k: {})
    db2 = tmp_path / "g.sqlite"
    fe.register_factor_population(db2, [(SH11_COND, 1, spec)])
    s2 = ResearchResultStore(db2)
    aid, _ = s2.begin_factor_evaluation_attempt(factor_id=fid, evaluation_id=fe.expected_evaluation_id(spec), origin="x")
    s2.finalize_factor_evaluation_attempt(aid, status="failed", expected_factor_id=fid,
                                          expected_evaluation_id=fe.expected_evaluation_id(spec), failure_reason="ValueError: boom")
    with pytest.raises(fe.FactorResumeRefusal):
        fe.resolve_factor(db2, tmp_path / "o", tmp_path / "r", None, SH11_COND, 1, spec, evaluate=lambda *a, **k: {})


def test_flipped_direction_is_refused_by_the_evaluator_and_changes_identity():
    flipped = fx.factor_spec(SH11_COND, 1, _mini_ctx("flip"), direction="higher_is_better")
    good = fx.factor_spec(SH11_COND, 1, _mini_ctx("flip"))
    assert flipped.compute_factor_id() != good.compute_factor_id()
    with pytest.raises(fx.FactorAuthorityRefusal):
        fe.evaluate_factor(Path("unused.sqlite"), Path("unused"), None, SH11_COND, 1, flipped)


# --------------------------------------------------------------------------- complete registration precedes evaluation
def _three_specs():
    conds = [c for c in pop.conditions(DECISIONS) if c["family"] == "SH11"][:3]
    ctx = _mini_ctx("reg")
    return [(c, 1, fx.factor_spec(c, 1, ctx)) for c in conds]


def test_partial_lazy_or_winner_only_factor_registration_cannot_be_evaluated(tmp_path):
    items = _three_specs()
    db = tmp_path / "f.sqlite"
    from mqk_research.factors.registry import register_factor
    for _c, _h, spec in items[:2]:
        register_factor(db, spec)
    with pytest.raises(fe.FactorPopulationRefusal, match="missing=1"):
        fe.require_registered_factor_population(db, items, allow_attempts=True)
    with pytest.raises(fe.FactorPopulationRefusal):
        fe.run_factors(db, tmp_path / "o", tmp_path / "r", None, items)              # refuses before any attempt
    register_factor(db, items[2][2])
    with pytest.raises(fe.FactorPopulationRefusal, match="freeze marker"):          # all registered, but no freeze marker
        fe.require_registered_factor_population(db, items, allow_attempts=True)
    fe.register_factor_population(db, items)
    fe.register_factor_population(db, items)                                          # idempotent on resume
    assert fe.require_registered_factor_population(db, items, allow_attempts=False)["registered"] == 3
    extra = fx.factor_spec(pop.conditions(DECISIONS)[0], 3, _mini_ctx("reg"))
    register_factor(db, extra)                                                         # an unfrozen factor appears
    with pytest.raises(fe.FactorPopulationRefusal, match="extra=1"):
        fe.require_registered_factor_population(db, items, allow_attempts=True)
    store = ResearchResultStore(db)
    assert not any(store.list_factor_evaluation_attempts(s.compute_factor_id()) for _c, _h, s in items)


def test_materialised_factor_population_must_match_the_frozen_coordinates_exactly():
    ctx = _mini_ctx("mat")
    frozen = pop.factor_population(DECISIONS)
    items = fe.materialize_specs(DECISIONS, ctx, frozen)
    assert len(items) == 1075 and {s.direction for _c, _h, s in items} == {"lower_is_better"}
    assert len({s.compute_factor_id() for _c, _h, s in items}) == 1075
    for bad in ({**frozen, "factor_count": 1074}, {**frozen, "coordinate_root": "0" * 64}):
        with pytest.raises(fe.FactorPopulationRefusal):
            fe.materialize_specs(DECISIONS, ctx, bad)
    shrunk = {**DECISIONS, "complement_handling": "EXCLUDE_COMPLEMENTS_BEFORE_FREEZE"}
    with pytest.raises(fe.FactorPopulationRefusal):                                    # 935 cannot satisfy a frozen 1,075
        fe.materialize_specs(shrunk, ctx, frozen)


# ----------------------------------------------------------------------------- Strategy registration / resume (idempotency)
def _mini_strategy(tmp_path, n=12):
    cells = pop.strategy_cells(DECISIONS, PID)[:n]
    ids = pop.strategy_ids(DECISIONS, PID)
    frozen = {"trial_count": n, "population_root": pop.sorted_root(c[3] for c in cells)}
    store = ResearchResultStore(tmp_path / "s.sqlite")
    return cells, ids, frozen, store


def test_partial_or_extra_strategy_registration_cannot_be_evaluated(tmp_path):
    cells, ids, frozen, store = _mini_strategy(tmp_path)
    store.register_hypothesis(hypothesis_id=rn.hypothesis_id("SH01"), experiment_id=pr.EXPERIMENT_ID, hypothesis_text=gr.FAMILY_NAMES["SH01"])
    rows = [{"trial_id": t, "experiment_id": pr.EXPERIMENT_ID, "hypothesis_id": rn.hypothesis_id(c["family"]),
             "strategy_id": f"{c['family']}:{c['config_id']}", "protocol_id": PID, "identity": gr.trial_identity(c, s, ids)} for _i, c, s, t in cells[:-1]]
    store.register_trials_bulk(rows)
    with pytest.raises(rn.CampaignRefusal, match="missing=1"):
        rn.require_registered_strategy_population(store, cells, frozen, PID, allow_attempts=True)
    with pytest.raises(rn.CampaignRefusal):                                           # the runner itself refuses before any attempt
        rn.run_strategy_chunks(store, None, cells, DECISIONS, {}, tmp_path / "out", frozen, PID, log=lambda *_: None)
    assert sum(d["attempts"] for d in store.trial_attempt_digest(pr.EXPERIMENT_ID).values()) == 0
    rn.register_strategy_population(store, cells, PID, ids, frozen)
    rn.register_strategy_population(store, cells, PID, ids, frozen)                  # a resumed run re-registers: idempotent
    assert rn.require_registered_strategy_population(store, cells, frozen, PID, allow_attempts=False)["registered"] == 12
    with pytest.raises(rn.CampaignRefusal):                                           # expanded population != frozen authority
        rn.require_registered_strategy_population(store, cells[:-1], frozen, PID, allow_attempts=True)
    with pytest.raises(rn.CampaignRefusal):
        rn.require_registered_strategy_population(store, cells, {**frozen, "trial_count": 11}, PID, allow_attempts=True)
    with pytest.raises(rn.CampaignRefusal, match="marker"):
        rn.require_registered_strategy_population(store, cells, frozen, "z" * 32, allow_attempts=True)
    store.begin_attempts_bulk([cells[0][3]], origin="t")
    with pytest.raises(rn.CampaignRefusal, match="attempts already exist"):
        rn.require_registered_strategy_population(store, cells, frozen, PID, allow_attempts=False)


def test_strategy_chunk_crash_and_rerun_is_new_attempts_of_the_same_trials(tmp_path, monkeypatch):
    monkeypatch.setattr(rn, "CHUNK_SIZE", 5)
    cells, ids, frozen, store = _mini_strategy(tmp_path)
    rn.register_strategy_population(store, cells, PID, ids, frozen)
    scope = sorted(set(c[2] for c in cells))
    U = fe.Universe2({s: ct.noise_bars(s, i + 1, 700, gap=0.02) for i, s in enumerate(scope + [x for x in ["SPY"] if x not in scope])})
    disp = {s: {"disposition": "ELIGIBLE"} for s in scope}
    real, calls = rn.evaluate_cell_row, {"n": 0}

    def flaky(U_, cell, decisions, dispositions):
        calls["n"] += 1
        if calls["n"] == 7:
            raise RuntimeError("infrastructure hiccup")
        return real(U_, cell, decisions, dispositions)

    monkeypatch.setattr(rn, "evaluate_cell_row", flaky)
    with pytest.raises(RuntimeError):
        rn.run_strategy_chunks(store, U, cells, DECISIONS, disp, tmp_path / "out", frozen, PID, log=lambda *_: None)
    monkeypatch.setattr(rn, "evaluate_cell_row", real)
    res = rn.run_strategy_chunks(store, U, cells, DECISIONS, disp, tmp_path / "out", frozen, PID, log=lambda *_: None)
    assert res["chunks_skipped_terminal"] == 1 and res["chunks_run"] == 2
    dig = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    assert len(dig) == 12 and set(dig) == {c[3] for c in cells}                      # a retry never mints a trial
    assert [dig[c[3]]["attempts"] for c in cells] == [1] * 5 + [2] * 5 + [1] * 2 and all(d["succeeded"] == 1 for d in dig.values())
    again = rn.run_strategy_chunks(store, U, cells, DECISIONS, disp, tmp_path / "out", frozen, PID, log=lambda *_: None)
    assert again["chunks_run"] == 0 and store.trial_attempt_digest(pr.EXPERIMENT_ID) == dig        # idempotent
    rows = rn.load_ledger(tmp_path / "out", cells)
    pr.assert_complete_ledger([c[3] for c in cells], [r["t"] for r in rows])
    with pytest.raises(pr.DenominatorShrink):                                         # winner-only ledger is refused
        pr.assert_complete_ledger([c[3] for c in cells], [r["t"] for r in rows if r["outcome"] == "QUALIFIED"])


def test_outputs_require_both_populations_complete_and_a_complete_ledger(tmp_path, monkeypatch, factor_runs):
    monkeypatch.setattr(rn, "CHUNK_SIZE", 3)
    cells, ids, frozen, store = _mini_strategy(tmp_path, 6)
    rn.register_strategy_population(store, cells, PID, ids, frozen)
    scope = sorted(set(c[2] for c in cells))
    U = fe.Universe2({s: ct.noise_bars(s, i + 1, 700, gap=0.02) for i, s in enumerate(scope + ["SPY"] if "SPY" not in scope else scope)})
    disp = {s: {"disposition": "ELIGIBLE"} for s in scope}
    run_dir = tmp_path / "run"
    rn.run_strategy_chunks(store, U, cells, DECISIONS, disp, run_dir, frozen, PID, max_chunks=1, log=lambda *_: None)
    doc = {"protocol_id": PID, "global_disclosure": pol.GLOBAL_DISCLOSURE}
    items, fdb, frec = factor_runs["items"], factor_runs["db"], factor_runs["rec_dir"]
    with pytest.raises(rn.CampaignIncomplete):                                         # half the Strategy ledger: no outputs
        rn.build_outputs(run_dir, cells, items, fdb, frec, DECISIONS, doc)
    assert not (run_dir / "strategy_edges.json").exists()
    rn.run_strategy_chunks(store, U, cells, DECISIONS, disp, run_dir, frozen, PID, log=lambda *_: None)
    out = rn.build_outputs(run_dir, cells, items, fdb, frec, DECISIONS, doc)
    assert out["fdr_status"] == "complete" and out["conditional_edges"] == 1                       # pos classed, neg not
    edges = json.loads((run_dir / "conditional_edges.json").read_text())
    assert edges[0]["direction"] == "lower_is_better" and edges[0]["executable_pnl"] is False and edges[0]["PROMOTION_AUTHORITY"] == "NONE"
    disc = json.loads((run_dir / "campaign_disclosure.json").read_text())
    assert disc["global_disclosure"]["census01_strategy_trials"] == 38_192 and disc["global_disclosure"]["census01_conditional_factors"] == 1_095
    dep = disc["complement_dependency"]
    assert dep["strategy_trials_registered"] == 6 and dep["strategy_trials_complement_tagged"] + dep["effective_independent_strategy_trials_estimate"] == 6
    assert dep["factors_registered"] == 2 and dep["effective_independent_factors_estimate"] == 2 - dep["factors_complement_related"] == 2
    assert disc["strategy_trials"] == 6 and disc["dsr_pbo"] == "DEFERRED_FULL_POPULATION" and disc["VALIDATION_STATUS"] == "NOT_VALIDATED"
    sedges = json.loads((run_dir / "strategy_edges.json").read_text())
    ledger = {r["t"]: r for r in rn.load_ledger(run_dir, cells)}
    assert {e["trial_id"] for e in sedges} == {t for t, r in ledger.items() if r["outcome"] == "QUALIFIED"}
    # a winner-only / shrunken factor record set is refused
    (frec / f"{items[1][2].compute_factor_id()}.json").unlink()
    with pytest.raises(fe.FactorResumeRefusal):
        rn.build_outputs(run_dir, cells, items, fdb, frec, DECISIONS, doc)


# ==================================================================== freeze-FIRST call order (the load-bearing proof)
class LoaderCalled(AssertionError):
    pass


@pytest.fixture(scope="module")
def frozen_repo(tmp_path_factory):
    return ct.make_frozen_repo(tmp_path_factory.mktemp("frozen"))


def _raising_loader(calls):
    def loader(*a, **k):
        calls.append("called")
        raise LoaderCalled("the loader was reached before / without a valid freeze")
    return loader


def _doc_variants(repo):
    good = json.loads((repo / "CENSUS02_PREDECLARATION.json").read_text())
    now = good["environment_identity"]
    bad_env = {**now, "pandas": now["pandas"] + ".1"}
    return {
        "missing": None,
        "proposed": {**good, "status": pr.STATUS_PROPOSED},
        "environment_mismatch": {**good, "environment_identity": bad_env, "protocol_id": pr.frozen_protocol_id(
            good["structural_protocol"], good["decisions"], good["behavior_source_manifest"], bad_env)},
        "decisions_not_approved": {**good, "decisions": {**good["decisions"], "ssr_handling": "FLAG_ONLY",
                                                         "etf_borrow_assumption": {**good["decisions"]["etf_borrow_assumption"],
                                                                                   "annual_borrow_fee_bps": 50.0}}},
        "attempts_open": {**good, "attempts_at_freeze": 1},
        "manifest_drift": {**good, "behavior_source_manifest": {**good["behavior_source_manifest"], "sources": {
            **good["behavior_source_manifest"]["sources"], pr.BEHAVIOR_SOURCES[0]: "0" * 64}}},
    }


@pytest.mark.parametrize("variant", ["missing", "proposed", "environment_mismatch", "decisions_not_approved", "attempts_open",
                                     "manifest_drift"])
def test_a_missing_or_invalid_freeze_never_reaches_the_loader(frozen_repo, tmp_path, variant):
    repo, committed = frozen_repo
    shutil_repo = tmp_path / "r"
    subprocess.run(["git", "clone", "-q", str(repo), str(shutil_repo)], check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@t"], cwd=shutil_repo, check=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=shutil_repo, check=True)
    doc = _doc_variants(repo)[variant]
    pre = shutil_repo / "CENSUS02_PREDECLARATION.json"
    if doc is None:
        pre.unlink()
    else:
        pre.write_text(json.dumps(doc, sort_keys=True))
    subprocess.run(["git", "add", "-A"], cwd=shutil_repo, check=True)
    subprocess.run(["git", "commit", "-qm", "variant"], cwd=shutil_repo, check=True, capture_output=True)
    calls, trace = [], []
    with pytest.raises(pr.FreezeRefusal):
        rn.run_campaign(repo=shutil_repo, predeclaration=pre, run_dir=tmp_path / "run", loader=_raising_loader(calls), trace=trace)
    assert calls == [] and trace == [] and not (tmp_path / "run").exists()                       # nothing opened, nothing read


def test_default_loader_is_the_guarded_entrance_and_receives_the_frozen_protocol_id(frozen_repo, tmp_path, monkeypatch):
    repo, pre = frozen_repo
    seen = {}

    class Sentinel(RuntimeError):
        pass

    def fake(data_dir, contract, protocol_id):
        seen.update(gate=dict(pr._GATE), protocol_id=protocol_id, contract=contract, data_dir=Path(data_dir))
        raise Sentinel

    monkeypatch.setattr(c2_data, "load_discovery_universe", fake)
    with pytest.raises(Sentinel):
        rn.run_campaign(repo=repo, predeclaration=pre, run_dir=tmp_path / "run")
    doc = json.loads(pre.read_text())
    assert seen["gate"]["protocol_id"] == seen["protocol_id"] == doc["protocol_id"]              # the gate had passed for THIS protocol
    assert seen["contract"] == doc["data_request_contract"] == pr.DATA_REQUEST_CONTRACT
    assert seen["data_dir"] == tmp_path / "run" / "data" and not (tmp_path / "run" / "registry_strategy.sqlite").exists()


def test_uncommitted_freeze_never_reaches_the_loader(frozen_repo, tmp_path):
    repo, _f = frozen_repo
    clone = tmp_path / "r"
    subprocess.run(["git", "clone", "-q", str(repo), str(clone)], check=True, capture_output=True)
    pre = clone / "CENSUS02_PREDECLARATION.json"
    pre.write_text(pre.read_text() + " ")                                                        # dirty vs HEAD
    calls = []
    with pytest.raises(pr.FreezeRefusal):
        rn.run_campaign(repo=clone, predeclaration=pre, run_dir=tmp_path / "run", loader=_raising_loader(calls))
    assert calls == []


@pytest.fixture(scope="module")
def campaign(frozen_repo, tmp_path_factory):
    """ONE valid synthetic end-to-end run: freeze gate -> synthetic loader -> complete registration -> bounded evaluation, with
    every provider / network / real-data path booby-trapped."""
    repo, pre = frozen_repo
    run_dir = tmp_path_factory.mktemp("run")
    seen_opens: list[str] = []
    real_open = builtins.open

    def spy_open(file, *a, **k):
        seen_opens.append(str(file))
        return real_open(file, *a, **k)

    def boom(*_a, **_k):
        raise AssertionError("a provider / network / real-data path was reached")

    mp = pytest.MonkeyPatch()
    for mod, names in ((dt1, ("acquire_symbol", "acquire_universe", "load_symbol_bars", "classify_eligibility", "load_alpaca_env")),
                        (c2_data.ce, ("build_bars_manifest", "load_bars"))):
        for n in names:
            mp.setattr(mod, n, boom)
    mp.setattr(socket.socket, "connect", boom)
    mp.setattr(builtins, "open", spy_open)
    mp.setattr(io, "open", spy_open)                                        # pathlib reads/writes go through io.open
    trace, calls = [], []
    uni = ct.synthetic_universe(700)

    def loader(data_dir, contract):
        calls.append((trace[:], contract))
        return uni

    try:
        result = rn.run_campaign(repo=repo, predeclaration=pre, run_dir=run_dir, loader=loader, max_strategy_chunks=1,
                                 max_factors=1, log=lambda *_: None, trace=trace)
    finally:
        mp.undo()
    return {"repo": repo, "pre": pre, "run_dir": run_dir, "trace": trace, "calls": calls, "result": result, "opens": seen_opens}


def test_freeze_gate_executes_before_the_loader_and_the_run_proceeds_on_synthetic_bars(campaign):
    assert campaign["trace"] == ["require_freeze", "loader", "fence", "registered_complete_populations", "strategy_evaluation",
                                 "factor_evaluation"]
    assert campaign["calls"][0][0] == ["require_freeze"]                        # at the instant the loader ran, the gate had passed
    assert campaign["calls"][0][1] == pr.DATA_REQUEST_CONTRACT                  # the loader receives the FROZEN contract
    opened = " ".join(campaign["opens"])
    assert "chunk_00000" in opened                                       # non-vacuous: the spy really observed file opens
    for forbidden in (".env", "research_bars", "status.json", "runs/alpha_edge_census_01"):
        assert forbidden not in opened, f"a real-data path was opened: {forbidden}"


def test_complete_population_registration_precedes_any_evaluation(campaign):
    store = ResearchResultStore(campaign["run_dir"] / "registry_strategy.sqlite")
    dig = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    doc = json.loads(campaign["pre"].read_text())
    cells = pop.strategy_cells(doc["decisions"], doc["protocol_id"])
    assert len(dig) == 9400 == doc["strategy_population"]["trial_count"] and set(dig) == {c[3] for c in cells}
    assert sum(d["attempts"] for d in dig.values()) == 500 and sum(1 for d in dig.values() if d["attempts"] == 0) == 8900
    fstore = ResearchResultStore(campaign["run_dir"] / "registry_factor.sqlite")
    assert len(fstore.list_factors(family=fx.FACTOR_FAMILY)) == 1075 == doc["factor_population"]["factor_count"]
    assert campaign["result"]["factors"]["evaluated_this_call"] == 1 and "outputs" not in campaign["result"]   # incomplete: no registry


def test_the_ledger_rows_are_typed_executable_and_carry_no_equity_cell(campaign):
    doc = json.loads(campaign["pre"].read_text())
    cells = pop.strategy_cells(doc["decisions"], doc["protocol_id"])
    rows = [json.loads(line) for line in (campaign["run_dir"] / "chunks" / "chunk_00000.jsonl").read_text().splitlines()]
    assert len(rows) == 500 and {r["s"] for r in rows} <= set(DECISIONS["etf_borrow_assumption"]["etf_short_scope"])
    assert {r["t"] for r in rows} == {c[3] for c in cells[:500]}
    assert all(r["outcome"] in st.OUTCOMES and "tags" in r and r["executable_pnl"] in (True, False) for r in rows)
    assert not _result_keys({"identity": [gr.trial_identity(cells[0][1], cells[0][2], pop.strategy_ids(doc["decisions"], doc["protocol_id"]))]})


REAL_RUN_DIR = ct.REAL_RUN_DIR
_tree_snapshot = ct.tree_snapshot
RAW_RUN_SCOPES = ("research-py/runs/", "research-py/experiments/alpha_edge_census_02/")
RAW_RUN_TRACKED_PATTERNS = ("/chunks/chunk_", "registry_strategy.sqlite", "registry_factor.sqlite", "/factor_records/",
                            "/factor_eval/", "research_bars", "corporate_actions", "/status.json")


def _tracked_raw_run_paths(repo: Path) -> list[str]:
    """Git-tracked paths that look like raw Census-02 run artifacts (run directory, chunk/registry/vendor-bar/status files).
    Committed evidence lives under experiments/alpha_edge_census_02/results/ and never matches."""
    out = subprocess.run(["git", "ls-files", "-z"], cwd=repo, capture_output=True, text=True, check=True).stdout
    return sorted(p for p in out.split("\0") if p.startswith(RAW_RUN_SCOPES[0]) or (
        p.startswith(RAW_RUN_SCOPES[1]) and any(t in "/" + p for t in RAW_RUN_TRACKED_PATTERNS)))


def test_importing_the_runner_and_cli_performs_no_io_and_leaves_the_run_directory_untouched(census02_run_dir_at_session_start):
    """State-independent: the real run directory may be absent (fresh checkout) or present (post-Result-#1); importing the Census-02
    modules (in this process and in a fresh interpreter) must not create it when absent nor modify a single file when present."""
    code = (
        "import socket, builtins, sys\n"
        "def boom(*a, **k): raise AssertionError('import-time network/open of a data path')\n"
        "socket.socket.connect = boom\n"
        "real_open = builtins.open\n"
        "def guard(f, *a, **k):\n"
        "    s = str(f)\n"
        "    if any(t in s for t in ('.env', 'research_bars', 'status.json', '/runs/')): raise AssertionError('import opened ' + s)\n"
        "    return real_open(f, *a, **k)\n"
        "builtins.open = guard\n"
        f"sys.path.insert(0, {str(EXP2)!r})\n"
        "import c2_runner, c2_data, run_census02, c2_factor_eval, c2_strategy, c2_population\n"
        "print('IMPORT_OK')\n")
    assert _tree_snapshot(REAL_RUN_DIR) == census02_run_dir_at_session_start   # pytest collection / in-process imports
    before = _tree_snapshot(REAL_RUN_DIR)
    p = subprocess.run([sys.executable, "-B", "-c", code], capture_output=True, text=True, cwd=EXP2.parents[1])
    assert p.returncode == 0 and "IMPORT_OK" in p.stdout, p.stderr[-600:]
    assert _tree_snapshot(REAL_RUN_DIR) == before


def test_the_tree_snapshot_detects_creation_and_modification(tmp_path):
    """Negative control for the import proof: the snapshot is RED on a created dir and on a modified file."""
    root = tmp_path / "run"
    assert _tree_snapshot(root) is None
    root.mkdir()
    created = _tree_snapshot(root)
    assert created == {} and created != None  # noqa: E711 - an empty dir is distinct from an absent one
    f = root / "chunks" / "chunk_00000.jsonl"
    f.parent.mkdir()
    f.write_text("a\n")
    one = _tree_snapshot(root)
    f.write_text("ab\n")
    assert _tree_snapshot(root) != one


def test_freeze_chronology_is_recorded_and_no_raw_run_artifact_is_tracked():
    """State-independent replacement for 'the run directory must not exist': the freeze recorded zero attempts / no results, no
    raw run artifact is tracked in Git, and any committed Result #1 evidence lives under results/ and binds to this freeze."""
    doc = json.loads(pr.PREDECLARATION_FILE.read_text(encoding="utf-8"))
    assert doc["attempts_at_freeze"] == 0 and doc["results_present_at_freeze"] is False and doc["status"] == pr.STATUS_FROZEN
    assert _tracked_raw_run_paths(pr.REPO) == []
    manifest = EXP2 / "results" / "discovery_result_01" / "RUN_MANIFEST.json"
    if manifest.exists():   # Result #1 occurs after the freeze: it binds to the frozen identity, it does not falsify it
        m = json.loads(manifest.read_text(encoding="utf-8"))
        assert m["protocol_id"] == doc["protocol_id"] and m["behavior_head"] == doc["behavior_head"]
        assert m["VALIDATION_STATUS"] == "NOT_VALIDATED" and m["PROMOTION_AUTHORITY"] == "NONE"
    assert not (EXP2 / "results" / "chunks").exists()


def test_the_tracked_raw_run_guard_flags_raw_artifacts_in_a_throwaway_repo(tmp_path):
    """Negative control: a tracked run directory / registry / vendor-bar file is flagged; committed results/ evidence is not."""
    def git(*a):
        subprocess.run(["git", *a], cwd=tmp_path, check=True, capture_output=True)
    git("init", "-q")
    ok = tmp_path / "research-py" / "experiments" / "alpha_edge_census_02" / "results" / "discovery_result_01"
    ok.mkdir(parents=True)
    (ok / "strategy_trial_ledger.jsonl").write_text("{}\n")
    (ok / "RUN_MANIFEST.json").write_text("{}\n")
    git("add", "-A")
    assert _tracked_raw_run_paths(tmp_path) == []
    for rel in ("research-py/runs/alpha_edge_census_02/registry_strategy.sqlite",
                "research-py/experiments/alpha_edge_census_02/chunks/chunk_00000.jsonl",
                "research-py/experiments/alpha_edge_census_02/data/SPY/research_bars.csv",
                "research-py/experiments/alpha_edge_census_02/data/SPY/status.json"):
        f = tmp_path / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text("x")
        git("add", "-f", rel)
        assert rel in _tracked_raw_run_paths(tmp_path), rel
        git("rm", "-q", "--cached", rel)
        assert _tracked_raw_run_paths(tmp_path) == []


def test_cli_freeze_is_immutable_policy_checked_and_read_only_on_data(tmp_path, monkeypatch):
    import run_census02 as cli
    target = tmp_path / "CENSUS02_PREDECLARATION.json"
    policy = tmp_path / "POLICY.json"
    policy.write_text(json.dumps(pol.policy_document(), sort_keys=True))
    stub = {"protocol_id": "x" * 32, "behavior_head": "h" * 40, "strategy_population": {"trial_count": 9400},
            "factor_population": {"factor_count": 1075}}
    monkeypatch.setattr(pr, "PREDECLARATION_FILE", target)
    monkeypatch.setattr(cli, "POLICY_FILE", policy)
    monkeypatch.setattr(pr, "build_predeclaration", lambda: stub)
    monkeypatch.setattr(rn, "run_campaign", lambda **k: pytest.fail("freeze must never run the campaign"))
    cli.main(["freeze"])
    assert json.loads(target.read_text()) == stub
    with pytest.raises(SystemExit, match="immutable"):
        cli.main(["freeze"])                                                       # a freeze is never overwritten
    target.unlink()
    policy.write_text(json.dumps({**pol.policy_document(), "decisions": {**pol.approved_decisions(), "ssr_handling": "FLAG_ONLY",
                                                                        "grammar_tiers": "H"}}))
    with pytest.raises(SystemExit, match="approved policy"):
        cli.main(["freeze"])                                                       # a policy artifact that differs from code
    assert not target.exists()
