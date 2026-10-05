"""Family ranking: the six keys decide in order, worst-slot treatment, >=4 evaluable, top-3 only,
no trial selection, and the Batch 03 execution stop (no result artifacts while the gate is closed)."""

from __future__ import annotations

import copy
import json
import os
import math
import sys
import types
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import family_ranking as fr  # noqa: E402

DECL = json.loads((HERE / "PREDECLARED_BATCH_03.json").read_text(encoding="utf-8"))
FAMILIES = [h["hypothesis_label"] for h in DECL["hypotheses"]]
SYMBOLS = DECL["universe"]["symbols"]


def slot(family, symbol, *, ok=True, dsr=0.5, alpha=1.0, robust=True, dd=0.1):
    return {"family": family, "symbol": symbol, "trial_id": f"{family}-{symbol}", "evaluable": ok,
            "dsr": dsr if ok else None, "alpha_pct": alpha if ok else None, "robustness_clear": robust and ok,
            "drawdown_improvement": dd if ok else None}


def population(overrides=None):
    """Every family identical and evaluable unless overridden: family -> kwargs or per-symbol dict."""
    overrides = overrides or {}
    slots = []
    for f in FAMILIES:
        for i, s in enumerate(SYMBOLS):
            kw = overrides.get(f, {})
            kw = kw.get(s, kw.get("*", {})) if kw and ("*" in kw or s in kw) else kw
            slots.append(slot(f, s, **kw))
    return slots


def rank(slots):
    return fr.rank_families(slots, families=FAMILIES, symbols=SYMBOLS)


def test_a_total_tie_falls_to_the_lowest_family_id_and_advances_exactly_three():
    r = rank(population())
    assert r["ranking"] == FAMILIES
    assert r["advanced_families"] == ["F01", "F02", "F03"]
    assert [f for f, o in r["family_outcomes"].items() if o == fr.ADVANCED] == ["F01", "F02", "F03"]
    assert all(o == fr.NOT_ADVANCED for f, o in r["family_outcomes"].items() if f not in r["advanced_families"])


@pytest.mark.parametrize("key,better,worse", [
    ("positive_alpha_symbols", {"alpha": 1.0}, {"alpha": -1.0}),
    ("median_dsr", {"dsr": 0.9}, {"dsr": 0.1}),
    ("median_net_alpha_pct", {"alpha": 5.0}, {"alpha": 1.0}),
    ("robustness_clear_trials", {"robust": True}, {"robust": False}),
    ("median_drawdown_improvement", {"dd": 0.5}, {"dd": -0.5}),
])
def test_each_key_decides_when_every_earlier_key_ties_and_beats_every_later_key(key, better, worse):
    # F10 is last by id; it wins ONLY through `key`, and an earlier-key loss cannot be bought back by later keys.
    base = {f: {} for f in FAMILIES}
    base["F10"] = better
    for f in FAMILIES[:-1]:
        base[f] = worse
    if key == "positive_alpha_symbols":
        # keep alpha magnitude comparable so only the count differs
        base["F10"] = {"alpha": 1.0}
        for f in FAMILIES[:-1]:
            base[f] = {"alpha": -1.0}
    r = rank(population(base))
    assert r["ranking"][0] == "F10", key
    assert "F10" in r["advanced_families"]


def test_key_order_an_earlier_key_beats_a_later_key():
    # F02: more positive-alpha symbols (key 1) but a far lower median DSR (key 2) than F01.
    slots = population()
    for s in slots:
        if s["family"] == "F01":
            s["alpha_pct"] = -1.0 if s["symbol"] == "SPY" else 1.0
            s["dsr"] = 0.99
        if s["family"] == "F02":
            s["dsr"] = 0.01
    r = rank(slots)
    assert r["ranking"].index("F02") < r["ranking"].index("F01")


def test_key2_median_dsr_outranks_key3_median_alpha_and_key4_outranks_key5():
    slots = population()
    for s in slots:
        if s["family"] == "F02":  # higher DSR, lower alpha than F01
            s["dsr"], s["alpha_pct"] = 0.9, 1.0
        if s["family"] == "F01":
            s["dsr"], s["alpha_pct"] = 0.2, 8.0
    assert rank(slots)["ranking"].index("F02") < rank(slots)["ranking"].index("F01")
    slots = population()
    for s in slots:
        if s["family"] == "F02":  # more robustness-clear trials, worse drawdown than F01
            s["robustness_clear"], s["drawdown_improvement"] = True, -0.5
        if s["family"] == "F01":
            s["robustness_clear"], s["drawdown_improvement"] = (s["symbol"] == "SPY"), 0.9
    assert rank(slots)["ranking"].index("F02") < rank(slots)["ranking"].index("F01")


def test_the_final_tie_break_is_the_family_id_not_input_order():
    r = fr.rank_families(list(reversed(population())), families=list(reversed(FAMILIES)), symbols=SYMBOLS)
    assert r["ranking"] == FAMILIES and r["advanced_families"] == ["F01", "F02", "F03"]


def test_non_evaluable_slots_count_as_worst_and_are_never_dropped():
    slots = population()
    for s in slots:
        if s["family"] == "F01" and s["symbol"] in ("SPY", "QQQ"):
            s.update(slot("F01", s["symbol"], ok=False))
    r = rank(slots)
    m = r["metrics"]["F01"]
    assert m["evaluable_trials"] == 4 and m["positive_alpha_symbols"] == 4
    assert m["median_dsr"] == 0.5  # 2 worst slots do not collapse the median of the remaining four middles
    # 3 worst slots (of 6) make a middle slot worst: the family is still rankable with 3? no: <4 evaluable.
    for s in slots:
        if s["family"] == "F01" and s["symbol"] == "IWM":
            s.update(slot("F01", "IWM", ok=False))
    r = rank(slots)
    assert r["family_outcomes"]["F01"] == fr.INSUFFICIENT
    assert "F01" not in r["advanced_families"] and "F01" not in r["ranking"]


def test_four_evaluable_is_the_floor_and_three_is_not_enough_even_when_best():
    weak = {f: {"dsr": 0.1, "alpha": -1.0} for f in FAMILIES if f != "F05"}
    slots = population({**weak, "F05": {"dsr": 0.99, "alpha": 9.0}})
    for s in slots:
        if s["family"] == "F05" and s["symbol"] in ("SMH", "XBI", "XLE"):
            s.update(slot("F05", s["symbol"], ok=False))
    assert rank(slots)["family_outcomes"]["F05"] == fr.INSUFFICIENT
    slots = population({**weak, "F05": {"dsr": 0.99, "alpha": 9.0}})
    for s in slots:
        if s["family"] == "F05" and s["symbol"] in ("XBI", "XLE"):
            s.update(slot("F05", s["symbol"], ok=False))
    r = rank(slots)
    assert r["family_outcomes"]["F05"] == fr.ADVANCED and r["ranking"][0] == "F05"


def test_fewer_than_three_rankable_families_advance_only_those():
    slots = population()
    for s in slots:
        if s["family"] not in ("F04", "F07"):
            s.update(slot(s["family"], s["symbol"], ok=False))
    r = rank(slots)
    assert r["advanced_families"] == ["F04", "F07"]
    assert sum(o == fr.INSUFFICIENT for o in r["family_outcomes"].values()) == 8


def test_a_missing_extra_or_duplicate_slot_is_an_unresolved_population_not_a_silent_drop():
    full = population()
    with pytest.raises(fr.UnresolvedPopulation):
        rank(full[:-1])
    with pytest.raises(fr.UnresolvedPopulation):
        rank(full + [slot("F01", "SPY")])
    extra = full + [slot("F11", "SPY")]
    with pytest.raises(fr.UnresolvedPopulation):
        rank(extra)
    swapped = copy.deepcopy(full)
    swapped[-1]["symbol"] = "SPY"
    with pytest.raises(fr.UnresolvedPopulation):
        rank(swapped)


def test_missing_drawdown_evidence_is_worst_on_key_five_only():
    slots = population()
    for s in slots:
        if s["family"] == "F01":
            s["drawdown_improvement"] = None
    r = rank(slots)
    assert r["metrics"]["F01"]["median_drawdown_improvement"] is None
    assert r["metrics"]["F01"]["evaluable_trials"] == 6
    assert r["ranking"][0] == "F02"  # F01 loses key 5; every earlier key ties


@pytest.mark.parametrize("bad", [True, "0.5", float("nan")])
def test_a_non_numeric_metric_is_worst_not_coerced(bad):
    slots = population()
    for s in slots:
        if s["family"] == "F01":
            s["dsr"] = bad
    assert rank(slots)["metrics"]["F01"]["median_dsr"] is None


NON_FINITE_OR_MALFORMED = [float("nan"), float("inf"), float("-inf"), True, False, "0.5", [0.5], {}]


def _ev_row(dsr=0.5, alpha=1.0, symbol="SPY", strategy="s"):
    return {"strategy": strategy, "symbol": symbol, "trial_id": f"{strategy}-{symbol}", "judge_status": "included",
            "dsr": dsr, "benchmark_evidence": {"alpha_pct": alpha}, "robustness_failed": [],
            "robustness_missing": []}


def _advantaged_family_cannot_win(field):
    """F10 carries `bad` core evidence on every symbol; every other family has weak-but-valid evidence.
    Were the bad number accepted as an extremely favourable input, F10 would win key 1 or key 2."""
    outcomes = []
    for bad in NON_FINITE_OR_MALFORMED + ([1.5, -0.1] if field == "dsr" else []):
        slots = []
        for f in FAMILIES:
            for sym in SYMBOLS:
                weak = {"dsr": 0.2, "alpha": -1.0}
                good = dict(weak, **{field: bad}) if f == "F10" else dict(weak)
                slots.append(fr.slot_from_evidence(
                    _ev_row(dsr=good["dsr"], alpha=good["alpha"], symbol=sym, strategy=f), f))
        r = rank(slots)
        outcomes.append((bad, r["family_outcomes"]["F10"], r["ranking"]))
    return outcomes


@pytest.mark.parametrize("field", ["dsr", "alpha"])
def test_non_finite_or_malformed_core_evidence_never_improves_a_family_rank(field):
    for bad, outcome, ranking in _advantaged_family_cannot_win(field):
        assert outcome == fr.INSUFFICIENT and "F10" not in ranking, (field, bad)
        assert len(ranking) == len(FAMILIES) - 1  # the slots stay in the 60-slot population, F10 unrankable


@pytest.mark.parametrize("bad", NON_FINITE_OR_MALFORMED + [1.5, -0.1])
def test_a_malformed_dsr_makes_the_slot_non_evaluable_but_keeps_it_in_the_population(bad):
    s = fr.slot_from_evidence(_ev_row(dsr=bad), "F01")
    assert s["evaluable"] is False and s["dsr"] is None and s["alpha_pct"] is None
    assert s["symbol"] == "SPY" and s["trial_id"] == "s-SPY"


@pytest.mark.parametrize("bad", NON_FINITE_OR_MALFORMED)
def test_a_malformed_alpha_makes_the_slot_non_evaluable_but_keeps_it_in_the_population(bad):
    s = fr.slot_from_evidence(_ev_row(alpha=bad), "F01")
    assert s["evaluable"] is False and s["dsr"] is None and s["alpha_pct"] is None
    assert s["symbol"] == "SPY" and s["trial_id"] == "s-SPY"


@pytest.mark.parametrize("dsr", [0.0, 0.5, 1.0, 0, 1])
@pytest.mark.parametrize("alpha", [-250.0, 0.0, 3.5, 1e9, -3])
def test_in_domain_finite_core_evidence_stays_evaluable(dsr, alpha):
    s = fr.slot_from_evidence(_ev_row(dsr=dsr, alpha=alpha), "F01")
    assert s["evaluable"] is True and s["dsr"] == float(dsr) and s["alpha_pct"] == float(alpha)


@pytest.mark.parametrize("field", ["dsr", "alpha_pct"])
@pytest.mark.parametrize("bad", [float("inf"), float("-inf"), float("nan")])
def test_a_pre_built_slot_with_a_non_finite_core_number_is_still_worst(field, bad):
    slots = population()
    for s in slots:
        if s["family"] == "F01":
            s[field] = bad
    m = rank(slots)["metrics"]["F01"]
    assert m["median_dsr" if field == "dsr" else "median_net_alpha_pct"] is None
    if field == "alpha_pct":
        assert m["positive_alpha_symbols"] == 0


def test_the_non_finite_controls_fail_under_a_nan_only_finiteness_mutant(monkeypatch):
    def controls():
        for field in ("dsr", "alpha"):
            for bad, outcome, _ in _advantaged_family_cannot_win(field):
                assert outcome == fr.INSUFFICIENT, (field, bad)
    controls()
    nan_only = types.SimpleNamespace(isfinite=lambda v: not math.isnan(v))
    monkeypatch.setattr(fr, "math", nan_only)
    with pytest.raises(AssertionError):
        controls()


def test_the_result_selects_no_trial_and_creates_no_promotion_candidate():
    r = rank(population())
    assert r["selected_trial_ids"] == [] and r["promotion_candidates"] == []
    assert r["promotion_candidate_created"] is False
    assert "PROMOTION" not in json.dumps(r["family_outcomes"]).upper().replace("NOT", "")
    assert all(set(v) <= {"evaluable_trials", "positive_alpha_symbols", "median_dsr", "median_net_alpha_pct",
                          "robustness_clear_trials", "median_drawdown_improvement"} for v in r["metrics"].values())


def test_slot_from_evidence_maps_the_batch_row_and_fails_closed():
    base = {"strategy": "x", "symbol": "SPY", "trial_id": "t1", "judge_status": "included", "dsr": 0.6,
            "benchmark_evidence": {"alpha_pct": 2.5}, "robustness_failed": [], "robustness_missing": []}
    s = fr.slot_from_evidence(base, "F01")
    assert s["evaluable"] and s["alpha_pct"] == 2.5 and s["dsr"] == 0.6 and s["robustness_clear"] is True
    assert s["drawdown_improvement"] is None
    for patch in ({"economic_failed": "boom"}, {"judge_status": "excluded:x"}, {"dsr": None},
                  {"benchmark_evidence": None}, {"benchmark_evidence": {}}):
        assert fr.slot_from_evidence({**base, **patch}, "F01")["evaluable"] is False, patch
    assert fr.slot_from_evidence({**base, "robustness_failed": ["a"]}, "F01")["robustness_clear"] is False
    assert fr.slot_from_evidence({**base, "robustness_missing": ["a"]}, "F01")["robustness_clear"] is False


def _bench(cand=2.0, base=5.0, imp=None, alpha=2.5):
    return {"alpha_pct": alpha, "candidate_max_drawdown_pct": cand, "benchmark_max_drawdown_pct": base,
            "drawdown_improvement_pct": base - cand if imp is None else imp}


def _row(bench, **extra):
    return {"strategy": "x", "symbol": "SPY", "trial_id": "t1", "judge_status": "included", "dsr": 0.6,
            "benchmark_evidence": bench, "robustness_failed": [], "robustness_missing": [], **extra}


def test_key5_is_read_only_from_engine_benchmark_drawdown_evidence():
    s = fr.slot_from_evidence(_row(_bench(2.0, 5.0)), "F01")
    assert s["drawdown_improvement"] == pytest.approx(3.0)  # benchmark minus candidate: lower drawdown is better
    assert fr.slot_from_evidence(_row(_bench(6.0, 5.0)), "F01")["drawdown_improvement"] == pytest.approx(-1.0)
    # a caller-supplied row-level number never substitutes for engine evidence
    s = fr.slot_from_evidence(_row({"alpha_pct": 2.5}, drawdown_improvement=9.9), "F01")
    assert s["evaluable"] and s["drawdown_improvement"] is None


@pytest.mark.parametrize("bad", [
    {"candidate_max_drawdown_pct": None},
    {"benchmark_max_drawdown_pct": None},
    {"drawdown_improvement_pct": None},
    {"drawdown_improvement_pct": -3.0},  # inverted direction (candidate minus benchmark)
    {"drawdown_improvement_pct": 3.5},  # not benchmark minus candidate
    {"candidate_max_drawdown_pct": float("nan")},
    {"benchmark_max_drawdown_pct": float("inf")},
    {"candidate_max_drawdown_pct": True},
    {"candidate_max_drawdown_pct": -1.0, "drawdown_improvement_pct": 6.0},
    {"benchmark_max_drawdown_pct": 120.0, "drawdown_improvement_pct": 118.0},
])
def test_partial_or_inconsistent_drawdown_evidence_is_worst_on_key_five_only(bad):
    s = fr.slot_from_evidence(_row({**_bench(2.0, 5.0), **bad}), "F01")
    assert s["evaluable"] and s["drawdown_improvement"] is None and s["alpha_pct"] == 2.5


def _key5_only_population():
    slots = population()
    for s in slots:
        s["drawdown_improvement"] = 2.0 if s["family"] == "F10" else 0.1
    return slots


def test_key5_mutations_are_killed(monkeypatch):
    # Control: F10 wins the key-5-only population (it is last by id, so only key 5 can lift it).
    assert rank(_key5_only_population())["ranking"][0] == "F10"
    real = fr.rank_key

    def inverted(fid, m):  # worse drawdown improvement ranked better
        k = list(real(fid, m))
        k[4] = -k[4]
        return tuple(k)

    def removed(fid, m):  # key 5 dropped
        k = list(real(fid, m))
        k[4] = 0
        return tuple(k)

    for mutant in (inverted, removed):
        monkeypatch.setattr(fr, "rank_key", mutant)
        assert rank(_key5_only_population())["ranking"][0] != "F10", mutant.__name__
        monkeypatch.setattr(fr, "rank_key", real)


def test_median_worst_semantics():
    n = fr.NEG_INF
    assert fr.median_worst([1, 2, 3, 4, 5, 6]) == 3.5
    assert fr.median_worst([n, n, 3, 4, 5, 6]) == 3.5
    assert fr.median_worst([n, n, n, 4, 5, 6]) == n
    assert fr.median_worst([n, 2, 3, 4, 5, 6]) == 3.5


def test_confirmation_template_is_prepared_only_with_the_declared_universe_and_no_substitution():
    conf = DECL["confirmation_preparation"]
    t = fr.confirmation_template(["F03", "F01", "F02"], conf["universe"])
    assert t["status"] == "PREPARED_NOT_REGISTERED" and t["registered"] is False
    assert t["history_verified"] is False and t["substitution_allowed"] is False
    assert len(t["future_trials"]) == conf["future_trials"] == 18
    assert {x["symbol"] for x in t["future_trials"]} == set(conf["universe"])


def test_family_ids_symbols_and_rule_match_the_declaration():
    rule = DECL["family_ranking"]
    assert len(FAMILIES) == 10 and len(SYMBOLS) == rule["trials_per_family"] == 6
    assert rule["min_evaluable_trials_per_family"] == 4


# ---- execution stop: BATCH03_PREDECLARED_NOT_EXECUTED ------------------------------------------

def test_the_closed_gate_leaves_no_batch03_result_artifacts_and_zero_attempts():
    gate = DECL["execution_gate"]
    assert gate["executable"] is False and gate["status"] == "BATCH03_PREDECLARED_NOT_EXECUTED"
    assert DECL["batch_stopping_rule"]["economic_attempts"] == 0
    run = HERE / DECL["run_dir"]
    produced = [p for name in ("batch_results.json", "batch_outcome.json", "family_ranking.json",
                               "trials_index.json", "judge", "scan", "backtest", "promotion_eligibility.json")
                if (p := run / name).exists()]
    assert produced == [], f"Batch 03 result artifacts exist while the gate is closed: {produced}"


def test_main_refuses_to_rank_while_the_gate_is_closed(monkeypatch):
    monkeypatch.setitem(os.environ, "MQK_M1_BATCH_DECLARATION", "PREDECLARED_BATCH_03.json")
    with pytest.raises(SystemExit, match="BATCH03_PREDECLARED_NOT_EXECUTED"):
        fr.main()
