"""The batch declaration records the capital-fraction sizing contract before any trial.

Absent block = historical fixed-quantity protocol (no flags, existing declarations
unchanged). A present block must be explicit and internally consistent; the Research
bridge (register/trials) receives the SAME sizing flags plus the explicit initial capital.
"""

from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import run_batch as rb  # noqa: E402

CORRECTED = json.loads((HERE / "PREDECLARED_BATCH_01_CORRECTED.json").read_text(encoding="utf-8"))
CF = "fixed_initial_capital_fraction_v1"
CF_BENCH = "capital_fraction_matched_passive_buy_hold_v1"


def declared(**block) -> dict:
    d = copy.deepcopy(CORRECTED)
    d["scanner_review"]["benchmark_policy"] = CF_BENCH
    d["capital_sizing"] = {"policy_id": CF, "allocation_fraction_bps": 2500,
                           "capital_basis": "native_backtest.initial_cash_micros", **block}
    return d


def test_historical_declarations_produce_no_sizing_flags():
    assert rb.sizing_args(CORRECTED) == []
    for name in ("PREDECLARED_BATCH_01.json", "PREDECLARED_BATCH_01_CORRECTED.json"):
        assert "capital_sizing" not in json.loads((HERE / name).read_text(encoding="utf-8"))


def test_valid_declaration_yields_explicit_flags_and_optional_caps():
    assert rb.sizing_args(declared()) == ["--sizing-policy", CF, "--allocation-fraction-bps", "2500"]
    assert rb.sizing_args(declared(max_target_qty=10, max_position_notional_usd=50000)) == [
        "--sizing-policy", CF, "--allocation-fraction-bps", "2500",
        "--max-target-qty", "10", "--max-position-notional-usd", "50000"]
    assert rb.sizing_args(declared(allocation_fraction_bps=1))[3] == "1"
    assert rb.sizing_args(declared(allocation_fraction_bps=10_000))[3] == "10000"


@pytest.mark.parametrize("bad", [
    {"allocation_fraction_bps": 0}, {"allocation_fraction_bps": 10_001}, {"allocation_fraction_bps": -1},
    {"allocation_fraction_bps": 25.0}, {"allocation_fraction_bps": "2500"}, {"allocation_fraction_bps": True},
    {"allocation_fraction_bps": None}, {"policy_id": "fixed_quantity_v1"}, {"policy_id": "unknown"},
    {"capital_basis": "current_equity"}, {"capital_basis": None},
    {"max_target_qty": 0}, {"max_target_qty": 1.5}, {"max_position_notional_usd": -5},
    {"surprise_field": 1},
])
def test_malformed_or_ambiguous_sizing_blocks_are_refused(bad):
    d = declared()
    d["capital_sizing"].update(bad)
    with pytest.raises(SystemExit, match="fail-closed"):
        rb.sizing_args(d)


def test_missing_required_fields_and_default_fraction_are_refused():
    for field in ("policy_id", "allocation_fraction_bps", "capital_basis"):
        d = declared()
        del d["capital_sizing"][field]
        with pytest.raises(SystemExit, match="fail-closed"):
            rb.sizing_args(d)
    with pytest.raises(SystemExit, match="fail-closed"):
        rb.sizing_args({**declared(), "capital_sizing": {}})
    with pytest.raises(SystemExit, match="fail-closed"):
        rb.sizing_args({**declared(), "capital_sizing": "fixed"})


def test_capital_fraction_requires_the_matching_benchmark_policy():
    for bench in ("capital_matched_exact_target_buy_hold_v1", None, "something_else"):
        d = declared()
        if bench is None:
            del d["scanner_review"]["benchmark_policy"]
        else:
            d["scanner_review"]["benchmark_policy"] = bench
        with pytest.raises(SystemExit, match="benchmark_policy"):
            rb.sizing_args(d)


def test_backtest_and_review_stages_carry_the_same_sizing_args():
    src = (HERE / "run_batch.py").read_text(encoding="utf-8")
    backtest = src[src.index("def stage_backtest"): src.index("def stage_finalize")]
    review = src[src.index("def stage_review"): src.index("def stage_summary")]
    assert "*sizing_args(DECL)" in backtest
    assert "sizing_args(DECL)" in review
    assert "if bench else []" in review


def test_research_bridge_carries_the_same_contract_and_never_refuses_a_valid_declaration():
    src = (HERE / "run_batch.py").read_text(encoding="utf-8")
    assert "_refuse_research_without_capital_fraction_bridge" not in src
    assert "capital_sizing is declared" not in src
    register = src[src.index("def stage_register"): src.index("def stage_trials")]
    trials = src[src.index("def stage_trials"): src.index("def stage_judge")]
    assert "*native_bridge_args(DECL)" in register and "capital_sizing=research_capital_sizing(DECL)" in register
    assert "*native_bridge_args(DECL)" in trials and "expected_capital_sizing=research_capital_sizing(DECL)" in trials
    # registration never runs the emitter or Backtest
    assert "native-signals" not in register and '"csv"' not in register


def test_native_bridge_args_and_research_contract_derive_from_the_declaration():
    assert rb.native_bridge_args(CORRECTED) == [] and rb.research_capital_sizing(CORRECTED) is None
    d = declared(max_target_qty=10)
    capital = int(d["native_backtest"]["initial_cash_micros"])
    assert rb.native_bridge_args(d) == [*rb.sizing_args(d), "--initial-cash-micros", str(capital)]
    assert rb.research_capital_sizing(d) == {
        "policy_id": CF, "allocation_fraction_bps": 2500, "initial_allocated_capital_micros": capital,
        "max_target_qty": 10, "max_position_notional_usd": None}
    # no default fraction/capital is ever substituted
    for field in ("allocation_fraction_bps",):
        bad = declared()
        del bad["capital_sizing"][field]
        with pytest.raises(SystemExit, match="fail-closed"):
            rb.native_bridge_args(bad)


def test_register_and_trials_validate_the_declaration_before_any_work(tmp_path):
    d = declared()
    d["capital_sizing"]["allocation_fraction_bps"] = 0
    f = tmp_path / "decl.json"
    f.write_text(json.dumps(d), encoding="utf-8")
    env = dict(os.environ, MQK_M1_BATCH_DECLARATION=str(f))
    out = subprocess.run([sys.executable, "run_batch.py", "check"], cwd=HERE, env=env,
                         capture_output=True, text=True)
    assert out.returncode != 0 and "allocation_fraction_bps" in out.stderr + out.stdout
