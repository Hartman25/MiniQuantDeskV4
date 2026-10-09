"""Cross-campaign search accounting for M1-KISS-EXT032-ETF-01: exact inventory from the frozen
declarations, the three counts kept apart, the broader-exposure disclosure, and the proof that the
existing judge cannot pool campaigns (so pooled DSR/PBO is BLOCKED, never fabricated)."""

from __future__ import annotations

import inspect
import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import search_accounting as sa  # noqa: E402
import test_kiss_ext032_registration_gate as gatehelp  # noqa: E402  (synthetic provenance helpers only)
import mqk_research.ml.native_signal_registry_integration as bridge  # noqa: E402
from mqk_research.ml import multiple_testing_judge as judge  # noqa: E402

DECL = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
DOC = (ROOT / "docs/research/M1_KISS_EXT032_SEARCH_ACCOUNTING.md").read_text(encoding="utf-8")


def test_the_verified_inventory_is_exactly_130_identities_115_operative_105_unique_pairs():
    inv = sa.prior_inventory()
    assert [r["registered_trial_identities"] for r in inv] == [5, 5, 5, 5, 5, 15, 15, 15, 60]
    assert [r["declaration"] for r in inv] == [d[0] for d in sa.PRIOR_DECLARATIONS]
    c = sa.counts(inv, new_trials=4)
    assert c["prior_registered_trial_identities_all"] == 130
    assert c["prior_operative"] == 115
    assert c["prior_unique_strategy_symbol_pairs"] == 105
    assert c["cumulative"] == {sa.OPERATIVE_RULE: 119, sa.RULE_ALL_IDENTITIES: 134, sa.RULE_STRICT_DEDUP: 109}
    assert c["cumulative_disclosed_search_count"] == 119 and c["operative_rule"] == sa.OPERATIVE_RULE


def test_the_operative_composition_is_the_operators_25_15_15_60():
    inv = {r["declaration"]: r for r in sa.prior_inventory() if r["counts_in_operative_rule"]}
    campaigns = [n for n in inv if n.startswith("PREDECLARED_CAMPAIGN")]
    assert len(campaigns) == 5 and sum(inv[n]["registered_trial_identities"] for n in campaigns) == 25
    assert inv["PREDECLARED_BATCH_01_CORRECTED.json"]["registered_trial_identities"] == 15
    assert inv["PREDECLARED_BATCH_02.json"]["registered_trial_identities"] == 15
    assert inv["PREDECLARED_BATCH_03.json"]["registered_trial_identities"] == 60
    assert "PREDECLARED_BATCH_01.json" not in inv


def test_every_prior_declaration_shares_the_one_exposed_window():
    acc = sa.build_accounting(DECL)
    assert acc["same_exposed_window"] == {"evaluation_start_utc": "2016-03-01T00:00:00Z", "test_months": 12,
                                          "holdout_months": 6, "expected_folds": 10}
    assert DECL["partition"]["evaluation_start_utc"] == acc["same_exposed_window"]["evaluation_start_utc"]


def test_superseded_and_voided_classifications_are_stated_by_their_evidence_documents():
    text = lambda p: (ROOT / p).read_text(encoding="utf-8")
    corrected = text("docs/research/M1_BATCH01_CORRECTED_RESULT.md")
    assert "superseded v1 evaluations" in corrected and "not counted as additional independent hypotheses" in corrected
    trend = text("docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md")
    assert "INVALID_FOR_STATED_HYPOTHESIS" in trend and "Campaign 01 — voided" in trend
    for name, _status, _counts, evidence in sa.PRIOR_DECLARATIONS:
        assert (ROOT / evidence).exists(), (name, evidence)


def test_removing_or_adding_a_prior_declaration_changes_the_count_so_no_denominator_is_hard_coded(monkeypatch):
    base = sa.counts(sa.prior_inventory(), 4)["cumulative_disclosed_search_count"]
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", sa.PRIOR_DECLARATIONS[:-1])  # drop Batch 03
    assert sa.counts(sa.prior_inventory(), 4)["cumulative_disclosed_search_count"] == base - 60
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS",
                        tuple((n, sa.STATUS_SUPERSEDED_V1, False, e) for n, _s, _, e in sa.PRIOR_DECLARATIONS))
    assert sa.counts(sa.prior_inventory(), 4)["prior_operative"] == 0


def test_the_new_hypothesis_is_genuinely_new_so_the_four_trials_add_four_in_every_rule():
    prior_strategies = {s for r in sa.prior_inventory() for s in r["strategy_ids"]}
    assert "pre_holiday_two_session_long_v1" not in prior_strategies
    prior_pairs = {tuple(p) for r in sa.prior_inventory() for p in r["pairs"]}
    new_pairs = {(t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]}
    assert len(new_pairs) == 4 and prior_pairs.isdisjoint(new_pairs)


def test_the_declared_accounting_block_equals_the_recomputed_accounting():
    acc = sa.build_accounting(DECL)
    block = DECL["search_accounting"]
    assert block["schema"] == sa.ACCOUNTING_SCHEMA == acc["schema"]
    assert block["cumulative_disclosed_search_count"] == acc["counts"]["cumulative_disclosed_search_count"] == 119
    assert block["prior_native_trial_identities_operative"] == 115 and block["new_trials"] == 4
    assert block["sensitivity_counts"] == acc["counts"]["cumulative"]
    assert block["operative_rule"] == sa.OPERATIVE_RULE
    assert block["pooled_statistics_support"] == "BLOCKED_UNSUPPORTED"
    assert "not deflated" in block["rule"] or "NOT deflated" in block["rule"] or "not deflated" in DOC


def test_the_document_states_the_same_numbers_and_the_blocked_pooling():
    for literal in ("**115**", "**119**", "| 130 | 134 |", "| 105 | 109 |", sa.OPERATIVE_RULE, "BLOCKED_UNSUPPORTED",
                    "CUMULATIVE_SEARCH_NOT_DEFLATED", "not counted as additional independent hypotheses"):
        assert literal in DOC, literal
    for name, *_ in sa.PRIOR_DECLARATIONS:
        assert name in DOC, name


def test_broader_exposure_is_disclosed_pinned_to_its_documents_and_never_counted():
    acc = sa.build_accounting(DECL)
    assert len(acc["broader_exposure"]) == len(sa.BROADER_EXPOSURE) == 8
    for row in acc["broader_exposure"]:
        assert row["counted_in_native_denominator"] is False
        assert row["pinned_literal"] in (ROOT / row["evidence_document"]).read_text(encoding="utf-8"), row["label"]
    for rel in sa.SEED_UNIVERSE_FILES:
        seed = json.loads((ROOT / rel).read_text(encoding="utf-8"))
        symbols = seed["symbols"] if isinstance(seed, dict) else seed
        symbols = [s["symbol"] if isinstance(s, dict) else s for s in symbols]
        assert len(symbols) == 88 and set(sa.CAMPAIGN_SYMBOLS) <= set(symbols), rel


def test_accounting_takes_no_result_and_makes_no_results_based_reduction():
    assert not inspect.signature(sa.counts).parameters.keys() - {"inventory", "new_trials"}
    assert not inspect.signature(sa.build_accounting).parameters.keys() - {"new_declaration", "root"}
    support = sa.pooled_statistics_support()
    assert support["no_results_based_denominator_reduction"] is True
    assert support["pooled_dsr"] is None and support["pooled_pbo"] is None and support["status"] == "BLOCKED_UNSUPPORTED"
    assert "reducing the cumulative denominator after seeing results" in DECL["forbidden_after_predeclaration"]
    assert "claiming a pooled cross-campaign DSR or PBO" in DECL["forbidden_after_predeclaration"]


# ------------------------------------------------ the judge cannot pool campaigns (proof, not assertion)

def _identity(tmp: Path, symbols, fingerprint="aa" * 32):
    rb = gatehelp.rb
    man = gatehelp.manifest(tmp, symbols=symbols)
    return bridge.build_native_signal_trial_identity(
        experiment_id=rb.EXPERIMENT, hypothesis_id="h", strategy_id="s", symbol=symbols[0], semantic_fingerprint=fingerprint,
        required_history_bars=2, bars_provenance=man,
        evaluation_start_utc=gatehelp.pd.Timestamp("2016-03-01T00:00:00Z"), test_months=12, holdout_months=6,
        economic_spec=rb._economic_spec(), capital_sizing=rb.research_capital_sizing(rb.DECL),
        stress_contract=rb.research_stress_contract(rb.DECL), canonical_timeframe_identity=True)[1]


def test_the_judge_comparison_key_separates_campaigns_with_different_universes(tmp_path):
    same_a = judge._comparison_key(_identity(tmp_path, ["SPY", "QQQ", "IWM", "DIA"]))
    same_b = judge._comparison_key(_identity(tmp_path, ["SPY", "QQQ", "IWM", "DIA"], fingerprint="bb" * 32))
    other = judge._comparison_key(_identity(tmp_path, ["SPY", "QQQ", "IWM", "XLE"]))
    assert same_a[0] == same_b[0], "strategies of one universe stay comparable (the in-experiment judge works)"
    assert same_a[0] != other[0], "a different symbol universe is an incompatible comparison scope"
    assert same_a[1]["bars_provenance"]["symbol_universe"] != other[1]["bars_provenance"]["symbol_universe"]


def test_earlier_campaigns_used_other_economic_protocols_and_sizing_than_this_one():
    inv = sa.prior_inventory()
    by = {r["declaration"]: r for r in inv}
    new_protocol = DECL["economic_protocol"]["signal_policy"]["direction_policy"]
    assert new_protocol == "native_exact_target_qty_v1"
    older = [r for r in inv if r["economic_protocol"] != new_protocol or not r["capital_fraction_sizing"]]
    assert {r["declaration"] for r in older} >= {"PREDECLARED_CAMPAIGN.json", "PREDECLARED_BATCH_01_CORRECTED.json",
                                                 "PREDECLARED_CAMPAIGN_DUAL_SMA_01.json"}
    # 5 campaigns (25) + Batch 01 v1 (15) + Batch 01 corrected (15); only Batch 02 and 03 share this protocol
    assert len(older) == 7 and sum(r["registered_trial_identities"] for r in older) == 55
    assert sum(r["registered_trial_identities"] for r in older if r["counts_in_operative_rule"]) == 40
    assert by["PREDECLARED_BATCH_03.json"]["economic_protocol"] == new_protocol
    assert by["PREDECLARED_BATCH_03.json"]["symbols"] != sorted(DECL["universe"]["symbols"])


def test_prior_return_series_are_not_committed_so_no_pooled_matrix_can_be_rebuilt():
    ignored = (ROOT / ".gitignore").read_text(encoding="utf-8")
    assert "research-py/runs/" in ignored
    committed = [p for p in (HERE).rglob("economic_daily_returns.csv")]
    assert committed == [], "per-trial daily return series must not be committed content"
    reasons = " ".join(sa.pooled_statistics_support()["reasons"])
    for phrase in ("incompatible_comparison_scope", "git-ignored", "different measurement processes",
                   "unsupported arithmetic substitution"):
        assert phrase in reasons


# ------------------------------------------------ build_accounting rejects a drifted population by itself (E1)

import copy  # noqa: E402


def _drift(mutate):
    d = copy.deepcopy(DECL)
    mutate(d)
    return d


@pytest.mark.parametrize("label,mutate,match", [
    ("missing slot", lambda d: d["universe"]["trials"].pop(), "trial slots differ"),
    ("duplicate pair", lambda d: d["universe"]["trials"].__setitem__(3, dict(d["universe"]["trials"][0], order=4)), "duplicate"),
    ("unexpected symbol", lambda d: (d["universe"]["symbols"].__setitem__(3, "XLE"),
                                     d["universe"]["trials"][3].update(symbol="XLE")), "unexpected symbols"),
    ("trial for an unlisted symbol", lambda d: d["universe"]["trials"][3].update(symbol="XLE"), "trial slots differ"),
    ("two hypotheses", lambda d: d["hypotheses"].append(copy.deepcopy(d["hypotheses"][0])), "hypothesis count"),
    ("zero hypotheses", lambda d: d["hypotheses"].clear(), "hypothesis count"),
    ("substituted hypothesis", lambda d: d["hypotheses"][0].update(strategy_id="turn_of_month_v1"), "not the contract's strategy"),
    ("max_trials 5", lambda d: d["universe"].update(max_trials=5), "max_trials"),
    ("max_trials 3", lambda d: d["universe"].update(max_trials=3), "max_trials"),
    ("extra variant trial", lambda d: d["universe"]["trials"].append(
        {"order": 5, "hypothesis_label": "H1", "strategy_id": "pre_holiday_two_session_long_v1_variant", "symbol": "SPY"}),
     "trial slots differ"),
    ("reordered slots", lambda d: d["universe"]["trials"].reverse(), "trial slots differ"),
    ("bad order fields", lambda d: [t.update(order=9) for t in d["universe"]["trials"]], "order fields"),
    ("window drift", lambda d: d["partition"].update(test_months=6), "not the prior exposed window"),
    ("declared count contradicts", lambda d: d["search_accounting"].update(cumulative_disclosed_search_count=118), "contradicts"),
    ("declared operative contradicts", lambda d: d["search_accounting"].update(prior_native_trial_identities_operative=105), "contradicts"),
    ("declared sensitivities contradict", lambda d: d["search_accounting"]["sensitivity_counts"].update(
        UNIQUE_STRATEGY_ID_SYMBOL_PAIRS=105), "contradicts"),
])
def test_build_accounting_rejects_a_drifted_new_population_on_its_own(label, mutate, match):
    with pytest.raises(ValueError, match=match):
        sa.build_accounting(_drift(mutate))


def test_the_shipped_declaration_is_accepted_and_the_checks_are_not_vacuous():
    acc = sa.build_accounting(DECL)
    assert acc["counts"]["new_trials"] == 4 and len(sa.validate_new_population(DECL)) == 4


def test_an_omitted_or_phantom_prior_campaign_or_an_invalid_status_is_rejected(monkeypatch):
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", sa.PRIOR_DECLARATIONS[:-1])
    with pytest.raises(ValueError, match="omitted from the accounting"):
        sa.build_accounting(DECL)
    monkeypatch.undo()
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", (*sa.PRIOR_DECLARATIONS, ("PREDECLARED_GHOST.json", sa.STATUS_REJECTED, True,
                                                                          sa.PRIOR_DECLARATIONS[0][3])))
    with pytest.raises(ValueError, match="accounted but absent"):
        sa.build_accounting(DECL)
    monkeypatch.undo()
    bad = list(sa.PRIOR_DECLARATIONS)
    bad[0] = (bad[0][0], "WINNER", True, bad[0][3])
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", tuple(bad))
    with pytest.raises(ValueError, match="invalid prior status"):
        sa.build_accounting(DECL)
    monkeypatch.undo()
    flipped = list(sa.PRIOR_DECLARATIONS)
    flipped[5] = (flipped[5][0], flipped[5][1], True, flipped[5][3])  # a superseded v1 evaluation forced into the operative count
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", tuple(flipped))
    with pytest.raises(ValueError, match="contradicts status"):
        sa.build_accounting(DECL)
    monkeypatch.undo()
    missing_doc = list(sa.PRIOR_DECLARATIONS)
    missing_doc[0] = (missing_doc[0][0], missing_doc[0][1], missing_doc[0][2], "docs/research/NOPE.md")
    monkeypatch.setattr(sa, "PRIOR_DECLARATIONS", tuple(missing_doc))
    with pytest.raises(ValueError, match="evidence document"):
        sa.build_accounting(DECL)


# ------------------------------------------------ what the numbers do and do not establish (E2)

def test_declared_identities_are_not_called_registry_verified():
    acc = sa.build_accounting(DECL)
    assert acc["identity_basis"]["independently_verified_in_a_registry"] is None
    assert acc["identity_basis"]["prior_trial_identities"] == "DECLARED_IN_FROZEN_DECLARATIONS"
    assert all(r["identity_basis"] == "DECLARED_IN_FROZEN_DECLARATION" and r["registry_verified"] is False
               for r in acc["prior_inventory"])


def test_disclosure_and_validation_are_distinct_and_validation_is_blocked_with_a_named_blocker():
    acc = sa.build_accounting(DECL)
    sv = acc["statistical_validation"]
    assert sv[sa.CUMULATIVE_SEARCH_DISCLOSED] == 119
    assert sv["cumulative_search_status"] == sa.CUMULATIVE_SEARCH_VALIDATION_BLOCKED != sa.CUMULATIVE_SEARCH_VALIDATED
    assert sv["statistical_acceptance_blocker"]["id"] == "SAB-1" and "not weakened" in sv["statistical_acceptance_blocker"]["effect"]
    assert "NOT deflated" in sv[sa.FOUR_TRIAL_JUDGE_RESULT]
    assert sa.cumulative_search_validation(acc) == sa.CUMULATIVE_SEARCH_VALIDATION_BLOCKED
    assert sa.cumulative_search_validation({"pooled_statistics_support": {"status": "SUPPORTED"}}) == \
        sa.CUMULATIVE_SEARCH_VALIDATION_BLOCKED, "only the exact verified status validates"
    assert sa.cumulative_search_validation({}) == sa.CUMULATIVE_SEARCH_VALIDATION_BLOCKED
