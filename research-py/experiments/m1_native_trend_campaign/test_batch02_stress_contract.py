"""The Batch 02 half-exposure stress is a recomputed 500 bps quantity, never a USD cap.

Pins the runner's validation of the stress contract and the exact CLI flags `finalize` sends, so
a cap-encoded "half exposure" (the 5,000 cap the exact-target replay refuses, or the historical
non-binding 25,000 cap) cannot be substituted for the frozen scenario.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "src"))

B2 = json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8"))
os.environ["MQK_M1_BATCH_DECLARATION"] = "PREDECLARED_BATCH_02.json"
try:
    SPEC = importlib.util.spec_from_file_location("run_batch_b2_under_test", HERE / "run_batch.py")
    rb = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(rb)
finally:
    del os.environ["MQK_M1_BATCH_DECLARATION"]
assert rb.DECL["batch_id"] == "m1_native_hypothesis_batch_02"


def decl() -> dict:
    return copy.deepcopy(B2)


def test_the_frozen_declaration_yields_the_500_bps_capital_fraction_plan():
    assert rb.stress_plan(B2) == {"mode": "capital_fraction",
                                  "scenario_id": "half_exposure_capital_fraction_500bps_v1",
                                  "allocation_fraction_bps": 500}
    assert rb.sizing_args(B2) == ["--sizing-policy", "fixed_initial_capital_fraction_v1",
                                  "--allocation-fraction-bps", "1000", "--max-position-notional-usd", "50000"]
    # The stress override changes ONLY the fraction: capital, policy and caps are the baseline's.
    assert rb.sizing_args(B2, 500) == ["--sizing-policy", "fixed_initial_capital_fraction_v1",
                                       "--allocation-fraction-bps", "500", "--max-position-notional-usd", "50000"]
    assert rb.native_bridge_args(B2, 500)[-2:] == ["--initial-cash-micros", "100000000000"]
    assert rb.research_capital_sizing(B2)["allocation_fraction_bps"] == 1000


@pytest.mark.parametrize("field,value", [
    ("stress_max_position_notional_usd", 5000), ("stress_max_position_notional_usd", 25000.0),
    ("stress_max_target_qty", 30)])
def test_a_usd_cap_can_never_encode_the_half_exposure(field, value):
    d = decl()
    d["robustness"]["p7a_p7b_stress"][field] = value
    with pytest.raises(SystemExit, match="recomputed quantity"):
        rb.stress_plan(d)


def test_a_capital_fraction_declaration_without_a_stress_sizing_block_is_refused():
    d = decl()
    del d["robustness"]["p7a_p7b_stress"]["stress_sizing"]
    d["robustness"]["p7a_p7b_stress"]["stress_max_position_notional_usd"] = 25000.0
    with pytest.raises(SystemExit, match="requires robustness.p7a_p7b_stress.stress_sizing"):
        rb.stress_plan(d)


@pytest.mark.parametrize("mutate,match", [
    (lambda s, c: s.update(allocation_fraction_bps=1000), "strictly below"),
    (lambda s, c: s.update(allocation_fraction_bps=1001), "strictly below"),
    (lambda s, c: s.update(allocation_fraction_bps=0), "strictly below"),
    (lambda s, c: s.update(allocation_fraction_bps=500.0), "strictly below"),
    (lambda s, c: s.update(allocation_fraction_bps=501), "nominal_entry_budget_micros"),
    (lambda s, c: s.update(initial_capital_micros=90_000_000_000), "same immutable capital"),
    (lambda s, c: s.update(policy_id="fixed_quantity_v1"), "capital-fraction policy"),
    (lambda s, c: s.update(is_a_trial=True), "never a trial"),
    (lambda s, c: s.update(surprise=1), "exactly the keys"),
    # The baseline replaced by the stress fraction (with its own consistent budget) leaves no reduction.
    (lambda s, c: c.update(allocation_fraction_bps=500, nominal_entry_budget_micros=5_000_000_000), "strictly below"),
])
def test_malformed_or_substituted_stress_sizing_is_refused(mutate, match):
    d = decl()
    mutate(d["robustness"]["p7a_p7b_stress"]["stress_sizing"], d["capital_sizing"])
    with pytest.raises(SystemExit, match=match):
        rb.stress_plan(d)


def test_fixed_quantity_declarations_keep_the_historical_cap_stress_and_refuse_a_stray_stress_sizing():
    old = json.loads((HERE / "PREDECLARED_BATCH_01_CORRECTED.json").read_text(encoding="utf-8"))
    assert rb.stress_plan(old) == {"mode": "cap"}
    bad = copy.deepcopy(old)
    bad["robustness"]["p7a_p7b_stress"]["stress_sizing"] = copy.deepcopy(B2["robustness"]["p7a_p7b_stress"]["stress_sizing"])
    with pytest.raises(SystemExit, match="requires a capital_sizing"):
        rb.stress_plan(bad)


class FakeCli:
    def __init__(self):
        self.calls: list[list[str]] = []

    def __call__(self, *argv: str) -> str:
        self.calls.append(list(argv))
        if argv[:2] == ("backtest", "native-fingerprint"):
            bps = argv[argv.index("--allocation-fraction-bps") + 1]
            return f"semantic_fingerprint={'1' if bps == '1000' else '5'}{'0' * 63}\nrequired_history_bars=1\n"
        return "ok=1\nrun_id=x\n"


def test_finalize_re_resolves_the_stress_quantity_and_sends_no_cap(tmp_path, monkeypatch):
    fake = FakeCli()
    monkeypatch.setattr(rb, "_run_cli", fake)
    monkeypatch.setattr(rb, "RUN", tmp_path)
    monkeypatch.setattr(rb, "INDEX", tmp_path / "trials_index.json")
    (tmp_path / "judge").mkdir()
    (tmp_path / "judge" / "judge_sha256.txt").write_text("a" * 64, encoding="utf-8")
    index = {rb.key(s, y): {"trial_id": "t", "economic_eval_id": "e", "backtest_run_id": "r"} for s, y in rb.TRIALS}
    rb._save_index(index)
    rb.stage_finalize(None)

    stress_calls = [c for c in fake.calls if c[:2] == ["backtest", "finalize-p7a-p7b-replay-stress"]]
    assert len(stress_calls) == 15
    for call in stress_calls:
        assert call[call.index("--stress-allocation-fraction-bps") + 1] == "500"
        assert call[call.index("--stress-sizing-scenario-id") + 1] == "half_exposure_capital_fraction_500bps_v1"
        assert "--stress-max-position-notional-usd" not in call and "--stress-max-target-qty" not in call
        assert call[call.index("--stress-execution-slippage-bps") + 1] == "15"
        assert call[call.index("--stress-execution-volatility-mult-bps") + 1] == "10"
        assert call[call.index("--max-drawdown-ceiling") + 1] == "0.4"
        assert call[call.index("--stress-sizing-expected-semantic-fingerprint") + 1] == "5" + "0" * 63
    emits = [c for c in fake.calls if c[:2] == ["backtest", "native-signals"]]
    assert len(emits) == 15
    for c in emits:  # the re-resolution runs at 500 bps on the unchanged capital and caps
        assert c[c.index("--allocation-fraction-bps") + 1] == "500"
        assert c[c.index("--initial-cash-micros") + 1] == "100000000000"
        assert c[c.index("--max-position-notional-usd") + 1] == "50000"
        assert "stress" in c[c.index("--out-dir") + 1] and c[c.index("--bars-path") + 1].endswith("bt_bars.csv")
    # the baseline robustness scenarios are untouched
    assert len([c for c in fake.calls if c[:2] == ["backtest", "finalize-robustness-sensitivity"]]) == 15
    assert len([c for c in fake.calls if c[:2] == ["backtest", "finalize-genuine-shuffled-placebo"]]) == 15


def test_batch02_declaration_registers_no_stress_contract_so_its_trial_ids_are_unchanged():
    # Batch 02 is a closed historical campaign: its 15 recorded trial identities carry no stress contract.
    assert "register_stress_contract" not in B2["robustness"]["p7a_p7b_stress"]
    assert rb.research_stress_contract(B2) is None


def test_a_declaration_can_opt_in_to_binding_its_stress_into_the_trial_identity():
    d = decl()
    d["robustness"]["p7a_p7b_stress"]["register_stress_contract"] = True
    assert rb.research_stress_contract(d) == {
        "schema_version": "p7a_p7b_stress_contract_v1",
        "scenario_id": "half_exposure_capital_fraction_500bps_v1", "allocation_fraction_bps": 500,
        "stress_execution_slippage_bps": 15, "stress_execution_volatility_mult_bps": 10,
        "max_drawdown_ceiling_bps": 4000}


@pytest.mark.parametrize("field,value,expected_key", [
    ("stress_execution_slippage_bps", 16, "stress_execution_slippage_bps"),
    ("stress_execution_volatility_mult_bps", 11, "stress_execution_volatility_mult_bps"),
    ("max_drawdown_ceiling", 0.35, "max_drawdown_ceiling_bps"),
])
def test_every_stress_input_of_the_declaration_reaches_the_registered_contract(field, value, expected_key):
    d = decl()
    d["robustness"]["p7a_p7b_stress"]["register_stress_contract"] = True
    d["robustness"]["p7a_p7b_stress"][field] = value
    got = rb.research_stress_contract(d)[expected_key]
    assert got == (3500 if field == "max_drawdown_ceiling" else value)


@pytest.mark.parametrize("ceiling", [0.40005, "0.4", True, None])
def test_a_ceiling_that_is_not_an_exact_bps_number_is_refused(ceiling):
    d = decl()
    d["robustness"]["p7a_p7b_stress"]["register_stress_contract"] = True
    d["robustness"]["p7a_p7b_stress"]["max_drawdown_ceiling"] = ceiling
    with pytest.raises(SystemExit, match="max_drawdown_ceiling"):
        rb.research_stress_contract(d)


@pytest.mark.parametrize("flag", ["yes", 1, "true"])
def test_the_opt_in_flag_must_be_a_literal_true(flag):
    d = decl()
    d["robustness"]["p7a_p7b_stress"]["register_stress_contract"] = flag
    with pytest.raises(SystemExit, match="register_stress_contract"):
        rb.research_stress_contract(d)


def test_opt_in_without_a_capital_fraction_declaration_is_refused():
    d = decl()
    d["robustness"]["p7a_p7b_stress"]["register_stress_contract"] = True
    del d["capital_sizing"]
    del d["robustness"]["p7a_p7b_stress"]["stress_sizing"]
    with pytest.raises(SystemExit, match="register_stress_contract"):
        rb.research_stress_contract(d)
