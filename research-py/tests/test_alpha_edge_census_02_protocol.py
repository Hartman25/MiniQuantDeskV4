"""Census-02 protocol scaffold: borrow capability, data fences, denominator guards, operator-decision schema and the
freeze guard. Synthetic fixtures only; no real Census-02 result is read or produced."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
from pathlib import Path

import pandas as pd
import pytest

EXP2 = Path(__file__).resolve().parents[1] / "experiments" / "alpha_edge_census_02"
sys.path.insert(0, str(EXP2))

import c2_borrow as bw  # noqa: E402
import c2_protocol as pr  # noqa: E402



# ============================================================================================ borrow capability
ASSUME = {"etf_short_scope": ["IWM", "SPY"], "annual_borrow_fee_bps": 50, "availability": bw.ASSUMPTION_FIXED["availability"],
          "recall": "NONE_ASSUMED", "short_rebate": "ZERO"}




@pytest.mark.parametrize("mut", [
    lambda a: a.pop("annual_borrow_fee_bps"), lambda a: a.update(annual_borrow_fee_bps=None),
    lambda a: a.update(annual_borrow_fee_bps=-1), lambda a: a.update(annual_borrow_fee_bps=float("inf")),
    lambda a: a.update(annual_borrow_fee_bps=True), lambda a: a.update(etf_short_scope=[]),
    lambda a: a.update(etf_short_scope=["SPY", "IWM"]), lambda a: a.update(etf_short_scope=["SPY", "SPY"]),
    lambda a: a.update(recall="MODELED"), lambda a: a.update(short_rebate="0.01"),
    lambda a: a.update(availability="EASY_TO_BORROW"), lambda a: a.update(extra=1)])
def test_etf_assumption_is_fail_closed(mut):
    a = copy.deepcopy(ASSUME)
    mut(a)
    with pytest.raises(bw.BorrowRefusal):
        bw.classify_evidence("SPY", a)



# ============================================================================================ fences
def _bars_at(dates):
    return pd.DataFrame({"end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in dates],
                         "open": 1.0, "high": 1.1, "low": 0.9, "close": 1.0, "volume": 1.0, "symbol": "T"})


@pytest.mark.parametrize("day", ["2024-01-02", "2024-06-03", "2025-01-02", "2025-12-01", "2026-03-02", "2026-09-01"])
def test_confirmation_contaminated_and_holdout_rows_are_fenced(day):
    with pytest.raises(pr.pt.PartitionBreach):
        pr.fence_bars(_bars_at(["2023-12-28", day]), what="t")


def test_fence_accepts_discovery_and_refuses_empty():
    assert len(pr.fence_bars(_bars_at(["2023-12-27", "2023-12-28"]), what="t")) == 2
    with pytest.raises(pr.pt.PartitionBreach):
        pr.fence_bars(_bars_at([]), what="t")


# ============================================================================================ denominators
def test_denominator_cannot_shrink_and_winner_only_registration_is_refused():
    pop = [f"ac02-{i:03d}" for i in range(10)]
    pr.assert_complete_ledger(pop, list(reversed(pop)))
    for bad in (pop[:-1], [pop[0]], pop + ["ac02-zzz"], pop + [pop[0]]):
        with pytest.raises(pr.DenominatorShrink):
            pr.assert_complete_ledger(pop, bad)
    winners = [{"trial_id": pop[0]}, {"trial_id": pop[3]}]
    with pytest.raises(pr.DenominatorShrink):
        pr.register_edges(winners, pop, [pop[0], pop[3]])                      # winners only
    assert pr.register_edges(winners, pop, pop) == winners
    with pytest.raises(pr.DenominatorShrink):
        pr.register_edges([{"trial_id": "ac02-stray"}], pop, pop)


# ============================================================================================ freeze guard
GOOD_DECISIONS = {
    "borrow_policy": "EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION", "etf_borrow_assumption": ASSUME,
    "grammar_tiers": "H+L", "complement_handling": "REGISTER_ALL_TAG_COMPLEMENTS",
    "benchmark_rule": "NET_POSITIVE_AND_SAME_DIRECTION_HOLD_ALPHA_POSITIVE",
    "multiple_testing_denominator": "LOCAL_WITH_GLOBAL_DISCLOSURE", "conditional_scope": "ALL_SEED_SYMBOLS",
    "ssr_handling": "FLAG_ONLY",
    "funnel_thresholds": {k: "OPERATOR_VALUE" for k in ("trade_count_confidence_band", "year_stability", "regime_concentration",
                                                       "parameter_neighborhood", "portfolio_mdd_worst5day")}}


def _git(repo, *a):
    subprocess.run(["git", *a], cwd=repo, check=True, capture_output=True)


def _frozen_doc(decisions=GOOD_DECISIONS, **over):
    s = pr.build_structural_protocol()
    doc = {"status": pr.STATUS_FROZEN, "attempts_at_freeze": 0, "structural_protocol": s, "decisions": decisions,
           "protocol_id": pr.frozen_protocol_id(s, decisions)}
    doc.update(over)
    return doc


@pytest.fixture
def repo(tmp_path):
    _git(tmp_path, "init", "-q")
    _git(tmp_path, "config", "user.email", "t@t")
    _git(tmp_path, "config", "user.name", "t")
    (tmp_path / "seed.txt").write_text("x")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-qm", "seed")
    return tmp_path


def _write_commit(repo, doc, commit=True):
    f = repo / "CENSUS02_PREDECLARATION.json"
    f.write_text(json.dumps(doc, sort_keys=True), encoding="utf-8")
    if commit:
        _git(repo, "add", "-A")
        _git(repo, "commit", "-qm", "freeze")
    return f


def test_freeze_guard_positive_control_then_every_refusal(repo):
    f = _write_commit(repo, _frozen_doc())
    assert pr.require_freeze(repo, f)["predeclaration"]["status"] == pr.STATUS_FROZEN      # positive control

    refusals = {
        "missing": lambda: pr.require_freeze(repo, repo / "nope.json"),
        "real_repo_without_freeze_file": lambda: pr.require_freeze(pr.REPO, pr.HERE / "DOES_NOT_EXIST.json"),
    }
    for name, fn in refusals.items():
        with pytest.raises(pr.FreezeRefusal):
            fn()
    for name, doc in {"proposed": _frozen_doc(status=pr.STATUS_PROPOSED), "attempts": _frozen_doc(attempts_at_freeze=1),
                      "bad_id": _frozen_doc(protocol_id="0" * 32),
                      "changed_structure": {**_frozen_doc(), "structural_protocol": {"x": 1}}}.items():
        f = _write_commit(repo, doc)
        with pytest.raises(pr.FreezeRefusal):
            pr.require_freeze(repo, f)
    f = _write_commit(repo, _frozen_doc(), commit=False)
    f.write_text(f.read_text() + " ")                                                        # dirty vs HEAD
    with pytest.raises(pr.FreezeRefusal):
        pr.require_freeze(repo, f)
    f.write_text(json.dumps(_frozen_doc(), sort_keys=True))
    _git(repo, "rm", "-q", "--cached", f.name)                                               # present but untracked
    with pytest.raises(pr.FreezeRefusal):
        pr.require_freeze(repo, f)


@pytest.mark.parametrize("mut", [
    lambda d: d.pop("benchmark_rule"), lambda d: d.update(benchmark_rule="LONG_HOLD"), lambda d: d.update(extra=1),
    lambda d: d.update(etf_borrow_assumption=None),
    lambda d: d.update(borrow_policy="NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY"),         # assumption present but disallowed
    lambda d: d.update(funnel_thresholds={"year_stability": "x"}),
    lambda d: d.update(etf_borrow_assumption={**ASSUME, "annual_borrow_fee_bps": None})])
def test_operator_decisions_must_be_complete_and_consistent(mut):
    d = copy.deepcopy(GOOD_DECISIONS)
    mut(d)
    with pytest.raises(pr.FreezeRefusal):
        pr.validate_decisions(d)
    assert pr.validate_decisions(copy.deepcopy(GOOD_DECISIONS))
    hypo = {**copy.deepcopy(GOOD_DECISIONS), "borrow_policy": "NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY", "etf_borrow_assumption": None}
    assert pr.validate_decisions(hypo)


def test_census02_modules_have_no_data_acquisition_path():
    forbidden = ("alpaca_historical", "load_symbol_bars", "extract_research_bars", "requests", "urllib", "runs/", ".env")
    for f in EXP2.glob("c2_*.py"):
        if f.name == "c2_mutation_proof.py":
            continue
        src = f.read_text(encoding="utf-8")
        for token in forbidden:
            assert token not in src, f"{f.name} references {token!r}"
