"""M1-KISS-EXT032-ETF-01: the prospective, NON-EXECUTABLE predeclaration (1 hypothesis, 4 trials,
0 attempts) and its stage guards. Nothing here reads market data, a registry of real trials or a
provider; the unauthorized-path tests run the real stage entry points in a subprocess and prove
they refuse and create nothing."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
CRATES = ROOT / "core-rs" / "crates"
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

NAME = "KISS"
DECL_NAME = "PREDECLARED_KISS_EXT032_ETF_01.json"
STRATEGY = "pre_holiday_two_session_long_v1"
SYMBOLS = ["SPY", "QQQ", "IWM", "DIA"]


def _load_run_batch(decl_name: str):
    os.environ["MQK_M1_BATCH_DECLARATION"] = decl_name
    try:
        spec = importlib.util.spec_from_file_location(f"run_batch_under_test_{decl_name}", HERE / "run_batch.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    finally:
        del os.environ["MQK_M1_BATCH_DECLARATION"]


rb = _load_run_batch(DECL_NAME)
RAW = json.loads((HERE / DECL_NAME).read_text(encoding="utf-8"))
B3 = json.loads((HERE / "PREDECLARED_BATCH_03.json").read_text(encoding="utf-8"))
sys.modules.pop("select_batch", None)
import select_batch  # noqa: E402


@pytest.fixture(autouse=True)
def restore_declaration():
    saved = copy.deepcopy(rb.DECL)
    saved_trials = list(rb.TRIALS)
    yield
    rb.DECL.clear()
    rb.DECL.update(saved)
    rb.TRIALS[:] = saved_trials


# ---------------------------------------------------------------- identity and cardinality

def test_campaign_identity_and_cardinality():
    assert RAW["campaign_id"] == "M1-KISS-EXT032-ETF-01"
    assert RAW["experiment"]["real_experiment_id"] == "M1-KISS-EXT032-ETF-01"
    assert len(RAW["hypotheses"]) == 1 and RAW["hypotheses"][0]["strategy_id"] == STRATEGY
    assert RAW["hypotheses"][0]["catalog_id"] == "EXT-032"
    assert [t["symbol"] for t in RAW["universe"]["trials"]] == SYMBOLS == RAW["universe"]["symbols"]
    assert [t["order"] for t in RAW["universe"]["trials"]] == [1, 2, 3, 4]
    assert RAW["universe"]["max_trials"] == 4 == len(rb.TRIALS)
    assert RAW["batch_stopping_rule"]["economic_attempts"] == 0
    assert RAW["expected_result_inventory"]["trials"] == 4
    assert RAW["expected_result_inventory"]["hypotheses"] == 1
    assert RAW["expected_result_inventory"]["attempts_before_execution"] == 0
    assert RAW["universe"]["additional_candidate_after_predeclaration"] == "forbidden"
    assert RAW["economic_outcome_status"] == "NONE_NOT_EXECUTED"


def test_the_frozen_trial_structure_check_accepts_four_and_refuses_three_and_five():
    rb._require_frozen_trial_structure()
    # three (drop one trial, keep max_trials=4)
    rb.DECL["universe"]["trials"].pop()
    rb.TRIALS[:] = [(t["strategy_id"], t["symbol"]) for t in rb.DECL["universe"]["trials"]]
    with pytest.raises(SystemExit, match="max_trials"):
        rb._require_frozen_trial_structure()
    # five (a fifth symbol/trial appended)
    rb.DECL.clear()
    rb.DECL.update(copy.deepcopy(RAW))
    rb.DECL["universe"]["symbols"].append("XLE")
    rb.DECL["universe"]["trials"].append({"order": 5, "hypothesis_label": "H1", "strategy_id": STRATEGY, "symbol": "XLE"})
    rb.TRIALS[:] = [(t["strategy_id"], t["symbol"]) for t in rb.DECL["universe"]["trials"]]
    with pytest.raises(SystemExit, match="max_trials"):
        rb._require_frozen_trial_structure()
    # duplicate trial
    rb.DECL.clear()
    rb.DECL.update(copy.deepcopy(RAW))
    rb.DECL["universe"]["trials"][3] = dict(rb.DECL["universe"]["trials"][2], order=4)
    rb.TRIALS[:] = [(t["strategy_id"], t["symbol"]) for t in rb.DECL["universe"]["trials"]]
    with pytest.raises(SystemExit, match="duplicate"):
        rb._require_frozen_trial_structure()


def test_the_runner_check_contract_validates_the_shipped_declaration():
    rb._require_exact_target_protocol()
    rb.sizing_args(rb.DECL)
    plan = rb.stress_plan(rb.DECL)
    assert plan == {"mode": "capital_fraction", "scenario_id": "half_exposure_capital_fraction_500bps_v1",
                    "allocation_fraction_bps": 500}
    assert rb.canonical_timeframe_identity(rb.DECL) is True
    contract = rb.research_stress_contract(rb.DECL)
    assert (contract["allocation_fraction_bps"], contract["stress_execution_slippage_bps"],
            contract["stress_execution_volatility_mult_bps"], contract["max_drawdown_ceiling_bps"]) == (500, 15, 10, 4000)


# ---------------------------------------------------------------- unchanged contracts (OD-5, OD-8)

@pytest.mark.parametrize("block", ["economic_protocol", "native_backtest", "capital_sizing", "benchmark", "robustness",
                                   "scanner_review", "execution_fidelity", "holdout"])
def test_economic_blocks_are_the_unchanged_batch03_contracts(block):
    assert RAW[block] == B3[block], f"{block} drifted from the accepted Batch 03 contract"


def test_promotion_policy_thresholds_are_unchanged_and_explicit():
    expected = {"MQK_PROMOTION_MIN_SHARPE": 0.5, "MQK_PROMOTION_MAX_MDD": 0.25, "MQK_PROMOTION_MIN_CAGR": 0.0,
                "MQK_PROMOTION_MIN_PROFIT_FACTOR": 1.05, "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT": 0.4,
                "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO": 0.5, "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING": 0.5,
                "MQK_RESEARCH_REQUIRE_NATIVE_SEMANTIC_BINDING": 1}
    got = {k: v for k, v in RAW["promotion_policy"].items() if k != "note"}
    assert got == expected
    assert {k: v for k, v in B3["promotion_policy"].items() if k != "note"} == expected
    assert RAW["benchmark"]["policy_id"] == "capital_fraction_matched_passive_buy_hold_v1"
    assert RAW["benchmark"]["min_alpha_pct"] == 0.0
    cs = RAW["capital_sizing"]
    assert (cs["policy_id"], cs["allocation_fraction_bps"], cs["initial_capital_micros"]) == (
        "fixed_initial_capital_fraction_v1", 1000, 100_000_000_000)
    st = RAW["robustness"]["p7a_p7b_stress"]
    assert (st["stress_sizing"]["scenario_id"], st["stress_sizing"]["allocation_fraction_bps"],
            st["stress_execution_slippage_bps"], st["stress_execution_volatility_mult_bps"],
            st["max_drawdown_ceiling"]) == ("half_exposure_capital_fraction_500bps_v1", 500, 15, 10, 0.4)
    assert st["stress_sizing"]["is_a_trial"] is False


def test_data_and_partition_follow_the_accepted_contract_with_the_holdout_excluded():
    d, b = RAW["data"], B3["data"]
    for key in ("path", "feed", "timeframe", "timeframe_identity", "adjustment", "start_utc", "end_utc",
                "completed_bars_only"):
        assert d[key] == b[key], key
    assert d["timeframe_identity"] == "canonical_semantic_v1" and d["adjustment"] == "all" and d["feed"] == "sip"
    p = RAW["partition"]
    assert {k: p[k] for k in ("evaluation_start_utc", "test_months", "holdout_months", "expected_folds")} == {
        k: B3["partition"][k] for k in ("evaluation_start_utc", "test_months", "holdout_months", "expected_folds")}
    assert "RESERVED / UNCONSUMED" in p["holdout_rule"] and "2026-03-01" in p["holdout_rule"]
    assert RAW["holdout"]["status"] == "RESERVED / UNCONSUMED"
    assert "OD-3" in d["holdout_window_fetch_disclosure"]
    assert "OPERATOR_ACKNOWLEDGEMENT_REQUIRED_AT_EXECUTION" in {u["status"] for u in RAW["unresolved_proof_capabilities"]}


def test_evidence_grade_is_exposed_development_and_never_independent():
    g = RAW["evidence_grade"]
    assert g["grade"] == "EXPOSED_DEVELOPMENT" and g["independent_confirmation"] is False
    assert {"INDEPENDENTLY_CONFIRMED", "OUT_OF_SAMPLE", "CONFIRMED"} <= set(g["forbidden_labels"])
    text = json.dumps(RAW)
    assert "INDEPENDENTLY_CONFIRMED" in text  # only as a forbidden label
    assert RAW["outcome_policy"]["automatic_selection"] is False
    assert RAW["outcome_policy"]["near_miss_selection"] == "FORBIDDEN"


# ---------------------------------------------------------------- engine, calendar and source identity

def test_hypothesis_matches_the_engine_source_and_registry():
    src = (CRATES / "mqk-strategy/src/engines/pre_holiday_two_session_long_v1.rs").read_text(encoding="utf-8")
    assert re.search(r'const NAME: &str = "([^"]+)"', src).group(1) == STRATEGY
    assert int(re.search(r"const REQUIRED_BARS: usize = (\d+);", src).group(1)) == RAW["hypotheses"][0]["required_history_bars"]
    assert int(re.search(r"const WINDOW_SESSIONS: i64 = (\d+);", src).group(1)) == RAW["hypotheses"][0]["parameters"]["WINDOW_SESSIONS"]
    assert int(re.search(r"const TIMEFRAME_SECS: i64 = ([\d_]+);", src).group(1).replace("_", "")) == 86400
    mod = (CRATES / "mqk-strategy/src/engines/mod.rs").read_text(encoding="utf-8")
    assert "pre_holiday_two_session_long_v1::NAME" in mod
    h = RAW["hypotheses"][0]
    assert h["restart_recovery"] == "BoundedHistoryReconstructible" and h["direction"] == "long_only_flat"
    assert h["no_variants"].startswith("no other window length")


def v2_content_hash_from_source() -> str:
    s = (CRATES / "mqk-integrity/src/sessions_v2.rs").read_text(encoding="utf-8")
    closures = re.findall(r"\((\d{4}), (\d+), (\d+), ClosureClass::(\w+)\)",
                          s[s.index("const CLOSURES"):s.index("const EARLY_CLOSES")])
    early = re.findall(r"\((\d{4}), (\d+), (\d+)\),", s[s.index("const EARLY_CLOSES"):s.index("/// Typed refusals")])
    out = "us_equity_regular_sessions_v2\ncoverage=2016-01-01..2028-12-31"
    for y, m, d, c in closures:
        out += f"\nclosure={int(y):04}-{int(m):02}-{int(d):02}|{c.lower()}"
    for y, m, d in early:
        out += f"\nearly_close={int(y):04}-{int(m):02}-{int(d):02}|13:00"
    return hashlib.sha256(out.encode()).hexdigest()


def test_calendar_block_binds_the_v2_contract_hash_and_preserves_v1():
    cal = RAW["calendar"]
    assert cal["contract_id"] == "us_equity_regular_sessions_v2" and cal["coverage_end_date"] == "2028-12-31"
    assert cal["expected_content_sha256"] == v2_content_hash_from_source()
    pinned_in_rust = re.search(r'CalendarContract::V2\.content_sha256\(\),\s*"([0-9a-f]{64})"',
                               (CRATES / "mqk-strategy/src/engines/session_calendar.rs").read_text(encoding="utf-8")).group(1)
    assert cal["expected_content_sha256"] == pinned_in_rust
    assert cal["frozen_predecessor"]["contract_id"] == "us_equity_regular_sessions_v1"
    assert cal["frozen_predecessor"]["expected_content_sha256"] == B3["calendar"]["expected_content_sha256"]
    assert cal["expected_content_sha256"] != cal["frozen_predecessor"]["expected_content_sha256"]
    # the historical declarations keep their calendar block byte-for-byte
    assert B3["calendar"]["contract_id"] == "us_equity_regular_sessions_v1"


def test_fingerprint_pins_are_well_formed_distinct_and_symbol_bound():
    per = RAW["strategy_fingerprints"]["per_symbol"]
    assert list(per) == SYMBOLS
    seen = set()
    for symbol, fp in per.items():
        for key in ("inner_semantic_fingerprint", "capital_fraction_wrapped_semantic_fingerprint"):
            assert re.fullmatch(r"[0-9a-f]{64}", fp[key]), (symbol, key)
            seen.add(fp[key])
        assert fp["inner_semantic_fingerprint"] != fp["capital_fraction_wrapped_semantic_fingerprint"]
    assert len(seen) == 8, "eight distinct fingerprints: four inner, four wrapped"
    assert RAW["strategy_fingerprints"]["wrapper_inputs"] == {
        "allocation_fraction_bps": 1000, "initial_allocated_capital_micros": 100_000_000_000}


def test_instrument_identity_pins_equal_the_accepted_registry_rows():
    registry = {r["symbol"]: r for r in json.loads((ROOT / "config/instruments/equities.json").read_text(encoding="utf-8"))}
    for symbol in SYMBOLS:
        row = registry[symbol]
        pin = RAW["universe"]["instrument_identity"][symbol]
        assert row["instrument_id"] == pin["instrument_id"] and row["instrument_kind"] == "etf" == pin["instrument_kind"]
        assert row["enabled"] is True and row["asset_class"] == "equity"
        assert hashlib.sha256(json.dumps(row, sort_keys=True, separators=(",", ":")).encode()).hexdigest() == pin["registry_row_sha256"]


def test_catalog_source_identity_matches_the_frozen_intake_and_excludes_other_rows():
    ledger = json.loads((ROOT / "docs/research/intake/external_idea_disposition_ledger_v1.json").read_text(encoding="utf-8"))
    row = next(r for r in ledger["rows"] if r["ext_id"] == "EXT-032")
    src = RAW["hypotheses"][0]["catalog_source"]
    assert src["workbook_sha256"] == ledger["source_sha256"]
    assert src["ledger_row_source_sha256"] == row["source_row_sha256"]
    assert row["primary_disposition"] == "M1_EQUITY_ETF_HYPOTHESIS" and row["population_tier"] == "A"
    assert row["direction"] == "LONG_FLAT" and row["calendar_bound"] is True
    # EXT-169 stays cataloged, outside the promotable population
    text = json.dumps(RAW)
    assert "EXT-169" not in text and "EXT-037" not in text and "EXT-044" not in text
    wb = ROOT / "docs/research/intake/MQD_External_Strategy_Idea_Catalog_2026-10-07.xlsx"
    assert hashlib.sha256(wb.read_bytes()).hexdigest() == src["workbook_sha256"]


# ---------------------------------------------------------------- non-executable gate and unauthorized paths

def test_gate_is_closed_with_the_named_blocker_and_refuses_every_other_value():
    gate = RAW["execution_gate"]
    assert gate["executable"] is False and gate["blocker"] == "OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED"
    assert gate["status"] == "KISS_EXT032_PREDECLARED_NOT_EXECUTED"
    with pytest.raises(SystemExit, match="OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED"):
        rb.require_executable_declaration(rb.DECL)
    for bad in (None, "true", 1, "yes", {"executable": True}):
        mutated = copy.deepcopy(RAW)
        mutated["execution_gate"]["executable"] = bad
        with pytest.raises(SystemExit):
            rb.require_executable_declaration(mutated)
    # only the literal True opens it
    opened = copy.deepcopy(RAW)
    opened["execution_gate"]["executable"] = True
    rb.require_executable_declaration(opened)


def test_a_graded_declaration_without_an_execution_gate_is_refused_not_treated_as_historical():
    stripped = copy.deepcopy(RAW)
    del stripped["execution_gate"]
    with pytest.raises(SystemExit, match="evidence_grade"):
        rb.require_executable_declaration(stripped)
    with pytest.raises(SystemExit, match="evidence_grade"):
        select_batch.require_declaration_runnable(stripped)
    # historical declarations (no gate, no grade) are unaffected
    rb.require_executable_declaration(json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8")))


def test_only_the_gate_may_differ_in_an_executable_reissue_of_the_shipped_declaration():
    reissued = copy.deepcopy(RAW)
    reissued["execution_gate"] = {"status": "AUTHORIZED", "executable": True, "blocker": None}
    rb.require_executable_declaration(reissued)
    # an executable re-issue still cannot be auto-selected: the outcome policy forbids it
    with pytest.raises(SystemExit, match="forbids automatic selection"):
        select_batch.require_declaration_runnable(reissued)


STAGES = ["check", "reuse_data", "fetch", "register", "gate", "trials", "judge", "backtest", "finalize", "review",
          "summary"]


def _hermetic_env(**extra) -> dict:
    """No provider credential can reach a spawned stage, whatever the host environment holds."""
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith(("ALPACA", "APCA", "TIINGO", "TWELVEDATA"))}
    env.update(extra)
    return env


def _require_still_unauthorized_before_spawning():
    """The spawn tests are only safe while the declaration is closed. If it was re-issued executable, fail
    BEFORE any stage could run: a stage must never be started by a test."""
    on_disk = json.loads((HERE / DECL_NAME).read_text(encoding="utf-8"))
    assert on_disk["execution_gate"]["executable"] is False, \
        "declaration is executable: the unauthorized-path spawn tests are obsolete and must not run stages"


@pytest.mark.parametrize("stage", STAGES)
def test_every_runner_stage_refuses_the_unauthorized_declaration_and_creates_nothing(stage, tmp_path):
    _require_still_unauthorized_before_spawning()
    run_dir = HERE / RAW["run_dir"]
    assert not run_dir.exists()
    env = _hermetic_env(MQK_M1_BATCH_DECLARATION=DECL_NAME, MQK_M1_CLI=str(tmp_path / "no-such-cli"))
    out = subprocess.run([sys.executable, str(HERE / "run_batch.py"), stage], capture_output=True, text=True,
                         env=env, cwd=tmp_path)
    assert out.returncode != 0
    assert "OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED" in out.stderr + out.stdout
    assert not run_dir.exists(), "a refused stage must not create the run directory"
    assert not (HERE / RAW["experiment"]["registry_db_relative_path"]).exists()


@pytest.mark.parametrize("script", ["select_batch.py", "summarize_batch.py", "family_ranking.py"])
def test_batch_judging_selection_and_ranking_entry_points_refuse(script, tmp_path):
    _require_still_unauthorized_before_spawning()
    env = _hermetic_env(MQK_M1_BATCH_DECLARATION=DECL_NAME)
    out = subprocess.run([sys.executable, str(HERE / script)], capture_output=True, text=True, env=env, cwd=tmp_path)
    assert out.returncode != 0
    blob = out.stderr + out.stdout
    assert "fail-closed" in blob or "KISS_EXT032_PREDECLARED_NOT_EXECUTED" in blob, blob[-400:]
    assert not (HERE / RAW["run_dir"]).exists()


def test_no_result_artifact_run_directory_or_registry_exists_and_no_paper_live_activation():
    assert not (HERE / "runs").exists() or not (HERE / RAW["run_dir"]).exists()
    assert not list(HERE.glob("**/batch_outcome.json")) and not list(HERE.glob("**/judge/judge.json"))
    assert RAW["paper_live"] == {"paper": "NOT ACTIVATED", "production_promotion_state": "NOT WRITTEN",
                                 "live": "NOT TOUCHED"}
    assert RAW["holdout"]["status"] == "RESERVED / UNCONSUMED"
    assert "Gate 3b (active_paper) is untouched" in RAW["evidence_grade"]["promotion_contract_note"]
    assert "hand promotion" in RAW["forbidden_after_predeclaration"]
    assert "Paper activation" in RAW["forbidden_after_predeclaration"] and "Live activation" in RAW["forbidden_after_predeclaration"]


def test_no_other_predeclaration_became_executable_or_changed_status():
    for path in sorted(HERE.glob("PREDECLARED_*.json")):
        d = json.loads(path.read_text(encoding="utf-8"))
        gate = d.get("execution_gate")
        if path.name == DECL_NAME:
            assert gate["executable"] is False
        elif path.name == "PREDECLARED_BATCH_03.json":
            assert gate["executable"] is True  # the operator's earlier, separate authorization
        else:
            assert gate is None, path.name


def test_stop_loss_policy_introduces_no_stop_and_lists_only_unexecuted_ideas():
    s = RAW["stop_loss_policy"]
    assert s["status"] == "NOT_INTRODUCED"
    assert s["unexecuted_future_ideas"] == ["fixed stop", "ATR stop", "ATR trailing stop"]
    h = RAW["hypotheses"][0]
    assert set(h["parameters"]) == {"WINDOW_SESSIONS"}
    assert "stop" in h["no_variants"]
    assert "adding a stop, trailing stop or any exit variant to this campaign" in RAW["forbidden_after_predeclaration"]
