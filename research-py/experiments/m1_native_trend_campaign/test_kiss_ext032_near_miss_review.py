"""The prepared, read-only results review (`m1_near_miss_review_v1`): every registered trial, fixed schema,
deterministic order, exact distances, no selection. Synthetic evidence rows only; no run directory of the
real campaign exists or is read."""

from __future__ import annotations

import copy
import json
import math
import random
import re
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))

import near_miss_review as nm  # noqa: E402

DECL = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
SYMBOLS = ["SPY", "QQQ", "IWM", "DIA"]
STRATEGY = "pre_holiday_two_session_long_v1"
WRAPPED = {s: DECL["strategy_fingerprints"]["per_symbol"][s]["capital_fraction_wrapped_semantic_fingerprint"] for s in SYMBOLS}


def good_row(symbol: str, **over) -> dict:
    row = {"strategy": STRATEGY, "symbol": symbol, "trial_id": f"trial-{symbol}", "attempt_index": 1,
           "semantic_fingerprint": WRAPPED[symbol], "net_return": 0.08, "sharpe": 0.8, "cagr": 0.05,
           "max_drawdown": 0.10, "profitable_months": 0.6, "position_agreement": 1.0, "dsr": 0.7,
           "judge_status": "included", "rust_total_return_pct": 8.0, "rust_trade_count": 40, "rust_profit_factor": 1.5,
           "rust_max_drawdown_pct": 12.0, "robustness_failed": [], "robustness_not_applicable": [],
           "robustness_missing": [], "stress_failed": [], "review_state": "paper_candidate",
           "reason_codes": "eligible_paper_candidate", "benchmark_evidence": {"alpha_pct": 1.2}}
    row.update(over)
    return row


JUDGE = {"judge_status": "evaluated", "pbo_result": {"status": "evaluated", "pbo": 0.2},
         "registry_population": {"registered_unique_trials": 4}, "excluded_trial_ids": []}
PROV = {"bars_provenance_manifest_present": True, "holdout_guard_post_passed": True}


def verdicts(symbols=SYMBOLS, passed=True):
    return {f"trial-{s}": {"passed": passed} for s in symbols}


def review(rows, judge=JUDGE, v=None, prov=PROV, decl=DECL):
    return nm.build_review(decl, rows, judge, verdicts=verdicts() if v is None else v, provenance=prov)


def trial(report, symbol):
    return next(t for t in report["trials"] if t["symbol"] == symbol)


def gate(t, name):
    return next(g for g in t["gates"] if g["gate"] == name)


# ------------------------------------------------------------------ completeness and no selection

def test_all_four_registered_trials_are_listed_in_declaration_order_and_none_is_selected():
    r = review([good_row(s) for s in SYMBOLS])
    assert [t["symbol"] for t in r["trials"]] == SYMBOLS and [t["order"] for t in r["trials"]] == [1, 2, 3, 4]
    assert r["status_counts"][nm.QUALIFIES] == 4, "four qualifiers are all listed, none is picked"
    assert r["selection"] is None and r["automatic_selection"] is False
    blob = json.dumps(r)
    for banned in ('"rank"', '"winner"', '"selected_trial', '"best"', '"recommended"'):
        assert banned not in blob, banned
    assert r["population"] == {"expected_trials": 4, "reviewed_trials": 4, "rows_supplied": 4, "duplicate_rows": [],
                               "unexpected_rows": [], "status": "COMPLETE"}


def test_a_missing_trial_row_is_reported_not_dropped_and_leaves_the_population_unresolved():
    r = review([good_row("SPY")])
    assert len(r["trials"]) == 4
    for s in ("QQQ", "IWM", "DIA"):
        t = trial(r, s)
        assert t["evaluability"] == "MISSING_EVIDENCE" and t["final_status"] == nm.UNQUALIFIED
        assert t["reason_codes"] == ["MISSING_EVIDENCE_ROW"]
    assert r["population"]["status"] == "UNRESOLVED"
    assert review([])["status_counts"][nm.UNQUALIFIED] == 4


def test_duplicate_and_unexpected_rows_are_surfaced_never_ignored():
    r = review([good_row(s) for s in SYMBOLS] + [good_row("SPY", trial_id="other")])
    assert trial(r, "SPY")["final_status"] == nm.INVALID and "SPY" and r["population"]["duplicate_rows"] == [f"{STRATEGY}/SPY"]
    assert r["population"]["status"] == "UNRESOLVED"
    extra = {**good_row("SPY"), "symbol": "XLE", "trial_id": "trial-XLE"}
    r = review([good_row(s) for s in SYMBOLS] + [extra])
    assert r["population"]["unexpected_rows"] == [f"{STRATEGY}/XLE"] and r["population"]["status"] == "UNRESOLVED"
    assert len(r["trials"]) == 4


def test_failed_attempts_and_judge_exclusions_stay_in_the_report():
    failed = good_row("QQQ", economic_failed="no native signals inside fold 3", judge_status="excluded:no_successful_attempt",
                      dsr=None)
    excluded = good_row("IWM", judge_status="excluded:degenerate_returns:zero_variance")
    r = review([good_row("SPY"), failed, excluded, good_row("DIA")])
    assert trial(r, "QQQ")["evaluability"] == "ECONOMIC_ATTEMPT_FAILED" and trial(r, "QQQ")["final_status"] == nm.UNQUALIFIED
    t = trial(r, "IWM")
    assert t["evaluability"] == "JUDGE_EXCLUDED" and t["final_status"] == nm.UNQUALIFIED
    assert t["reason_codes"] == ["JUDGE_EXCLUDED_DEGENERATE_RETURNS_ZERO_VARIANCE"]
    assert trial(r, "SPY")["final_status"] == nm.QUALIFIES
    assert len(r["trials"]) == 4 and r["population"]["reviewed_trials"] == 4 and r["population"]["status"] == "COMPLETE"
    assert r["status_counts"][nm.UNQUALIFIED] == 2, "failures are counted, not dropped"


# ------------------------------------------------------------------ exact distances

def test_distances_are_exact_and_equality_passes_for_every_comparator():
    cases = {  # gate -> (failing value, exact shortfall, passing-at-equality value)
        "sharpe": (0.49, 0.01, 0.5), "dsr": (0.45, 0.05, 0.5), "cagr": (-0.002, 0.002, 0.0),
        "profitable_months": (0.35, 0.05, 0.4), "profit_factor": (1.0, 0.05, 1.05), "trade_count": (4, 1, 5),
        "total_return_pct": (-0.5, 0.5, 0.0), "max_drawdown_pct": (26.0, 1.0, 25.0), "cost_aware_alpha_pct": (-0.3, 0.3, 0.0),
    }
    field = {"sharpe": "sharpe", "dsr": "dsr", "cagr": "cagr", "profitable_months": "profitable_months",
             "profit_factor": "rust_profit_factor", "trade_count": "rust_trade_count",
             "total_return_pct": "rust_total_return_pct", "max_drawdown_pct": "rust_max_drawdown_pct"}
    for name, (bad, shortfall, edge) in cases.items():
        def with_value(v):
            if name == "cost_aware_alpha_pct":
                return good_row("SPY", benchmark_evidence={"alpha_pct": v}, review_state="paper_candidate" if v >= 0 else "rejected")
            over = {field[name]: v}
            scanner = {"total_return_pct", "max_drawdown_pct", "trade_count", "profit_factor"}
            if name in scanner:
                over["review_state"] = "paper_candidate" if v == edge else "rejected"
            return good_row("SPY", **over)
        g = gate(trial(review([with_value(bad)]), "SPY"), name)
        assert g["status"] == "FAIL" and math.isclose(g["shortfall"], shortfall, abs_tol=1e-12), name
        assert g["margin"] < 0
        ok = gate(trial(review([with_value(edge)]), "SPY"), name)
        assert ok["status"] == "PASS" and ok["shortfall"] == 0.0 and ok["margin"] == 0.0, f"equality passes: {name}"
    sharpe = gate(trial(review([good_row("SPY", sharpe=0.49)]), "SPY"), "sharpe")
    assert math.isclose(sharpe["relative_shortfall"], 0.02, rel_tol=1e-9) and sharpe["threshold"] == 0.5
    alpha = gate(trial(review([good_row("SPY", benchmark_evidence={"alpha_pct": -0.3}, review_state="rejected")]), "SPY"),
                 "cost_aware_alpha_pct")
    assert alpha["threshold"] == 0.0 and alpha["relative_shortfall"] is None


def test_unavailable_or_non_finite_values_fail_closed_and_are_invalid_evidence_not_a_near_miss():
    for bad in (None, float("nan"), float("inf"), "n/a", True):
        t = trial(review([good_row("SPY", cagr=bad)]), "SPY")
        g = gate(t, "cagr")
        assert g["status"] == "NOT_AVAILABLE" and g["passed"] is False and g["margin"] is None
        assert t["final_status"] == nm.INVALID and "REQUIRED_METRIC_UNAVAILABLE:cagr" in t["reason_codes"]


def test_the_report_carries_the_pbo_and_denominator_beside_every_dsr():
    r = review([good_row(s) for s in SYMBOLS])
    assert r["judge"]["pbo"] == 0.2 and r["judge"]["judge_status"] == "evaluated"
    assert r["cumulative_disclosed_search_count"] == 119
    for t in r["trials"]:
        assert gate(t, "pbo")["value"] == 0.2 and gate(t, "pbo")["threshold"] == 0.5
        assert set(t["caveats"]) == {"EXPOSED_DEVELOPMENT_NOT_INDEPENDENT", "CUMULATIVE_SEARCH_NOT_DEFLATED"}
        assert t["evidence_grade"] == "EXPOSED_DEVELOPMENT"
        assert t["data_partition"]["holdout"] == "RESERVED_NOT_FORMALLY_CONSUMED__ACCESS_INCIDENT_PENDING_ADJUDICATION"
    assert r["independent_confirmation"] is False


# ------------------------------------------------------------------ the five final statuses

def test_quantitative_only_failures_are_near_misses_with_every_failed_gate_listed():
    row = good_row("SPY", sharpe=0.45, dsr=0.4, benchmark_evidence={"alpha_pct": -0.1}, review_state="rejected",
                   reason_codes="non_positive_alpha")
    t = trial(review([row]), "SPY")
    assert t["final_status"] == nm.NEAR_MISS
    assert t["failed_gates"] == ["cost_aware_alpha_pct", "sharpe", "dsr"]
    assert t["reason_codes"] == ["QUANTITATIVE_GATE_FAILED:cost_aware_alpha_pct", "QUANTITATIVE_GATE_FAILED:sharpe",
                                 "QUANTITATIVE_GATE_FAILED:dsr"]


@pytest.mark.parametrize("label,over,code", [
    ("robustness scenario failed", {"robustness_failed": ["placebo_temporal_offset"]}, "ROBUSTNESS_FAILED:placebo_temporal_offset"),
    ("native stress failed", {"stress_failed": ["p7a_p7b_economic_replay_stress"]}, "NATIVE_STRESS_FAILED:p7a_p7b_economic_replay_stress"),
    ("dsr missing from the judge", {"dsr": None}, "JUDGE_DSR_PBO_NOT_EVALUABLE"),
])
def test_structural_failures_are_unqualified_even_when_every_number_passes(label, over, code):
    t = trial(review([good_row("SPY", **over)]), "SPY")
    assert t["final_status"] == nm.UNQUALIFIED and code in t["reason_codes"], label


def test_a_judge_that_did_not_evaluate_makes_every_trial_unqualified():
    j = {**JUDGE, "judge_status": "partially_evaluable", "pbo_result": {"status": "not_evaluable", "pbo": None}}
    r = review([good_row(s) for s in SYMBOLS], judge=j)
    assert all(t["final_status"] == nm.UNQUALIFIED for t in r["trials"])
    assert all("JUDGE_DSR_PBO_NOT_EVALUABLE" in t["reason_codes"] for t in r["trials"])


@pytest.mark.parametrize("label,over,code", [
    ("fidelity below floor", {"position_agreement": 0.949}, "EXECUTION_FIDELITY_BELOW_FLOOR_OR_UNAVAILABLE"),
    ("fidelity unavailable", {"position_agreement": None}, "EXECUTION_FIDELITY_BELOW_FLOOR_OR_UNAVAILABLE"),
    ("required robustness evidence missing", {"robustness_missing": ["execution_delay_stress"]}, "REQUIRED_ROBUSTNESS_EVIDENCE_MISSING"),
    ("backtest halted", {"reason_codes": "halted", "review_state": "rejected"}, "BACKTEST_HALTED"),
    ("review says candidate but its metrics fail", {"rust_profit_factor": 0.9}, "REVIEW_STATE_INCONSISTENT_WITH_ITS_OWN_METRICS"),
    ("review rejects while its metrics pass", {"review_state": "rejected"}, "REVIEW_STATE_INCONSISTENT_WITH_ITS_OWN_METRICS"),
    ("fingerprint differs from the declaration", {"semantic_fingerprint": "00" * 32}, "IDENTITY_FINGERPRINT_DIFFERS_FROM_DECLARATION"),
    ("trial id missing", {"trial_id": ""}, "IDENTITY_TRIAL_ID_MISSING"),
])
def test_invalid_execution_identity_or_safety_evidence(label, over, code):
    t = trial(review([good_row("SPY", **over)]), "SPY")
    assert t["final_status"] == nm.INVALID and code in t["reason_codes"], label


def test_fidelity_floor_is_inclusive_at_the_declared_value():
    assert DECL["execution_fidelity"]["floor"] == 0.95
    assert trial(review([good_row("SPY", position_agreement=0.95)]), "SPY")["final_status"] == nm.QUALIFIES
    assert trial(review([good_row("SPY", position_agreement=0.9499999)]), "SPY")["final_status"] == nm.INVALID


def test_every_gate_passing_is_not_enough_without_provenance_and_the_canonical_verdict():
    rows = [good_row("SPY")]
    t = trial(review(rows, prov={"bars_provenance_manifest_present": True, "holdout_guard_post_passed": False}), "SPY")
    assert t["final_status"] == nm.INSUFFICIENT and "PROVENANCE_UNVERIFIED:holdout_guard_post_passed" in t["reason_codes"]
    t = trial(review(rows, prov=None), "SPY")
    assert t["final_status"] == nm.INSUFFICIENT
    t = trial(review(rows, v={}), "SPY")
    assert t["final_status"] == nm.INSUFFICIENT and "CANONICAL_PROMOTION_VERDICT_MISSING" in t["reason_codes"]
    t = trial(review(rows, v=verdicts(passed=False)), "SPY")
    assert t["final_status"] == nm.INSUFFICIENT and "CANONICAL_PROMOTION_VERDICT_NEGATIVE" in t["reason_codes"]
    t = trial(review(rows), "SPY")
    assert t["final_status"] == nm.QUALIFIES and t["reason_codes"] == ["ALL_GATES_PASSED"]
    assert t["structural"]["canonical_promotion_verdict"] is True


def test_the_five_statuses_are_exactly_the_declared_classes_and_every_status_is_reachable():
    assert tuple(nm.STATUSES) == tuple(DECL["near_miss_review"]["classes"][i] for i in (0, 1, 2, 3, 4)) or \
        set(nm.STATUSES) == set(DECL["near_miss_review"]["classes"])
    rows = [good_row("SPY"),
            good_row("QQQ", sharpe=0.1, dsr=0.1, review_state="paper_candidate"),
            good_row("IWM", robustness_failed=["symbol_leave_one_out"]),
            good_row("DIA", position_agreement=0.5)]
    r = review(rows)
    seen = {t["final_status"] for t in r["trials"]}
    assert seen == {nm.QUALIFIES, nm.NEAR_MISS, nm.UNQUALIFIED, nm.INVALID}
    assert trial(review([good_row("SPY")], prov=None), "SPY")["final_status"] == nm.INSUFFICIENT


# ------------------------------------------------------------------ determinism, schema, read-only

def test_the_report_is_deterministic_independent_of_input_order_and_has_a_fixed_schema():
    rows = [good_row("SPY"), good_row("QQQ", sharpe=0.3, review_state="paper_candidate"), good_row("IWM"),
            good_row("DIA", economic_failed="x", judge_status="excluded:no_successful_attempt")]
    base = nm.canonical_json(review(rows))
    for seed in range(8):
        shuffled = rows[:]
        random.Random(seed).shuffle(shuffled)
        assert nm.canonical_json(review(shuffled)) == base
    assert nm.report_sha256(review(rows)) == nm.report_sha256(review(copy.deepcopy(rows)))
    r = review(rows)
    assert r["schema"] == "m1_near_miss_review_v1"
    assert list(r) == ["schema", "campaign_id", "evidence_grade", "independent_confirmation",
                       "cumulative_disclosed_search_count", "judge", "population", "status_counts", "trials", "selection",
                       "automatic_selection", "notes"]
    stamps = set(re.findall(r"\d{4}-\d\d-\d\dT[\d:]+Z?", json.dumps(r)))
    assert stamps <= {"2016-03-01T00:00:00Z"}, "only the declared partition start; no run timestamp"
    md = nm.render_markdown(r)
    assert md == nm.render_markdown(review(copy.deepcopy(rows))) and "No trial is selected" in md


def test_the_declared_class_list_and_gate_thresholds_match_the_unchanged_policy():
    assert set(DECL["near_miss_review"]["classes"]) == set(nm.STATUSES)
    thresholds = {g: nm._threshold(DECL, ref) for g, _c, ref, _s in nm.GATES}
    assert thresholds == {"cost_aware_alpha_pct": 0.0, "total_return_pct": 0.0, "sharpe": 0.5, "dsr": 0.5, "pbo": 0.5,
                          "cagr": 0.0, "max_drawdown_pct": 25.0, "profit_factor": 1.05, "profitable_months": 0.4,
                          "trade_count": 5.0}
    comparators = {g: c for g, c, _r, _s in nm.GATES}
    assert comparators["pbo"] == "<=" and comparators["max_drawdown_pct"] == "<="
    assert all(c == ">=" for g, c in comparators.items() if g not in ("pbo", "max_drawdown_pct"))
    assert DECL["promotion_policy"]["MQK_PROMOTION_MAX_MDD"] == 0.25 and nm.SCANNER_MAX_DRAWDOWN_PCT == 25.0


def test_scanner_constants_equal_the_rust_default_policy():
    src = (ROOT / "core-rs/crates/mqk-backtest/src/strategy_scan_review.rs").read_text(encoding="utf-8")
    body = src[src.index("impl Default for StrategyScanReviewPolicy"):]
    body = body[:body.index("}\n    }\n}")]
    def num(field):
        return float(re.search(rf"{field}: ([\d.]+),", body).group(1))
    assert num("min_trade_count") == nm.SCANNER_MIN_TRADES
    assert num("min_total_return_pct") == nm.SCANNER_MIN_TOTAL_RETURN_PCT
    assert num("max_drawdown_pct") == nm.SCANNER_MAX_DRAWDOWN_PCT
    assert num("min_profit_factor") == nm.SCANNER_MIN_PROFIT_FACTOR
    assert num("min_alpha_pct") == DECL["benchmark"]["min_alpha_pct"]


_AUDIT = {"on": False, "events": []}
_WATCHED = {"open", "socket.connect", "subprocess.Popen", "os.system", "os.remove", "os.rename", "os.mkdir",
            "shutil.rmtree", "sqlite3.connect"}


def _audit_hook(event, args):
    if _AUDIT["on"] and event in _WATCHED:
        _AUDIT["events"].append((event, args[0] if args else None))


sys.addaudithook(_audit_hook)  # audit hooks cannot be removed; this one is inert unless the flag is set


def test_building_the_review_opens_no_file_and_makes_no_network_or_process_call():
    rows = [good_row(s) for s in SYMBOLS]
    nm.render_markdown(review(rows))  # warm any lazy imports outside the audited window
    _AUDIT["events"].clear()
    _AUDIT["on"] = True
    try:
        r = review(rows)
        nm.render_markdown(r)
        nm.report_sha256(r)
        nm.canonical_json(r)
    finally:
        _AUDIT["on"] = False
    assert _AUDIT["events"] == []
    # and the audit really works: an open inside the window is seen
    _AUDIT["on"] = True
    try:
        open(HERE / "near_miss_review.py", encoding="utf-8").close()
    finally:
        _AUDIT["on"] = False
    assert any(e[0] == "open" for e in _AUDIT["events"])


def test_the_cli_refuses_the_non_executable_declaration_and_writes_nothing():
    import os
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith(("ALPACA", "APCA"))}
    env["MQK_M1_BATCH_DECLARATION"] = "PREDECLARED_KISS_EXT032_ETF_01.json"
    on_disk = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
    assert on_disk["execution_gate"]["executable"] is False, "spawn test is obsolete once the declaration is executable"
    out = subprocess.run([sys.executable, str(HERE / "near_miss_review.py")], capture_output=True, text=True, env=env)
    assert out.returncode != 0 and "no authorized execution to review" in out.stderr + out.stdout
    assert not (HERE / DECL["run_dir"]).exists()


def test_the_cli_reads_a_run_and_never_writes_into_it(tmp_path, monkeypatch):
    """A re-issued declaration on a private run directory: the review reads, prints, and refuses an --out inside the run."""
    decl = copy.deepcopy(DECL)
    decl["execution_gate"] = {"status": "AUTHORIZED", "executable": True, "blocker": None}
    (tmp_path / "decl.json").write_text(json.dumps(decl), encoding="utf-8")
    run = tmp_path / decl["run_dir"]
    (run / "judge").mkdir(parents=True)
    (run / "data").mkdir()
    (run / "batch_results.json").write_text(json.dumps([good_row(s) for s in SYMBOLS]), encoding="utf-8")
    (run / "judge" / "judge.json").write_text(json.dumps(JUDGE), encoding="utf-8")
    (run / "data" / "research_bars_provenance.json").write_text("{}", encoding="utf-8")
    monkeypatch.setattr(nm, "HERE", tmp_path)
    monkeypatch.setenv("MQK_M1_BATCH_DECLARATION", "decl.json")
    before = sorted(p.relative_to(run).as_posix() for p in run.rglob("*"))
    nm.main(["--out", str(tmp_path / "review.json")])
    assert sorted(p.relative_to(run).as_posix() for p in run.rglob("*")) == before, "the run directory is untouched"
    report = json.loads((tmp_path / "review.json").read_text(encoding="utf-8"))
    assert report["status_counts"][nm.INSUFFICIENT] == 4, "no verdict file and no holdout-guard report -> insufficient"
    assert (tmp_path / "review.md").exists()
    with pytest.raises(SystemExit, match="never written into the run directory"):
        nm.main(["--out", str(run / "review.json")])


def test_every_field_the_review_reads_is_written_by_the_evidence_table_producer():
    """Interface pin: the review's inputs are the rows summarize_batch.py writes and the benchmark evidence the
    scanner emits. A renamed field fails here instead of silently turning into 'unavailable'."""
    producer = (HERE / "summarize_batch.py").read_text(encoding="utf-8")
    needed = {"strategy", "symbol", "trial_id", "semantic_fingerprint", "sharpe", "cagr", "profitable_months",
              "position_agreement", "dsr", "judge_status", "rust_total_return_pct", "rust_trade_count",
              "rust_profit_factor", "rust_max_drawdown_pct", "robustness_failed", "robustness_not_applicable",
              "robustness_missing", "stress_failed", "review_state", "reason_codes", "benchmark_evidence",
              "economic_failed"}
    for key in sorted(needed):
        assert f'"{key}"' in producer, key
    scanner = (ROOT / "core-rs/crates/mqk-backtest/src/strategy_scanner.rs").read_text(encoding="utf-8")
    block = scanner[scanner.index("pub struct ScanCapitalFractionBenchmarkEvidence"):]
    block = block[:block.index("\n}\n")]
    assert "pub alpha_pct: f64" in block
    review_states = (ROOT / "core-rs/crates/mqk-backtest/src/strategy_scan_review.rs").read_text(encoding="utf-8")
    assert '"paper_candidate"' in review_states or "PaperCandidate" in review_states
    judge = (ROOT / "research-py/src/mqk_research/ml/multiple_testing_judge.py").read_text(encoding="utf-8")
    for key in ('"judge_status"', '"pbo_result"', '"registry_population"', '"excluded_trial_ids"'):
        assert key in judge, key
