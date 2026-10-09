"""Read-only, deterministic results review for a graded M1 campaign (`m1_near_miss_review_v1`).

Prepared BEFORE any economic execution; populated only after authorized execution. It reviews EVERY
registered trial of the declaration (failures, non-evaluable attempts and missing evidence included),
in the declaration's trial order, against the unchanged gates, and reports for each quantitative gate
the value, the threshold and the exact signed distance. It selects nothing: there is no rank, no
winner, no "best near-miss", and a research failure is never turned into a candidate by formatting.

Pure over its inputs (the evidence table `batch_results.json` written by summarize_batch.py, the
batch judge, the declaration, an optional canonical `evaluate_promotion` verdict file and optional
provenance facts). It reads no market data, opens no registry, writes nothing into the run directory
and makes no network call.

Final status per trial (first match wins):

  INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE      identity differs from the declaration, duplicate row, execution
                                                     fidelity below the floor, halted/blocked backtest, required
                                                     robustness/stress evidence missing, inconsistent review state
  STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE          missing row, failed attempt, judge could not include the trial,
                                                     DSR/PBO not evaluable, a robustness or stress scenario failed
  INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION  every gate passes but the provenance facts or the canonical
                                                     Promotion verdict are absent or negative
  QUANTITATIVE_ECONOMIC_NEAR_MISS                    evaluable and valid; every failed gate is quantitative
  QUALIFIES_UNDER_EVERY_APPLICABLE_GATE              every gate passes, the Promotion verdict passed, provenance present

`QUALIFIES...` is not independent confirmation: every row carries the caveats of its evidence grade
(EXPOSED_DEVELOPMENT) and of the cumulative search count, which the four-trial judge does not deflate.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCHEMA = "m1_near_miss_review_v1"

INVALID = "INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE"
UNQUALIFIED = "STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE"
INSUFFICIENT = "INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION"
NEAR_MISS = "QUANTITATIVE_ECONOMIC_NEAR_MISS"
QUALIFIES = "QUALIFIES_UNDER_EVERY_APPLICABLE_GATE"
STATUSES = (QUALIFIES, NEAR_MISS, UNQUALIFIED, INSUFFICIENT, INVALID)

# Scanner review policy (mqk_backtest::StrategyScanReviewPolicy::default, pinned to the Rust source by test).
SCANNER_MIN_TRADES = 5
SCANNER_MIN_TOTAL_RETURN_PCT = 0.0
SCANNER_MAX_DRAWDOWN_PCT = 25.0
SCANNER_MIN_PROFIT_FACTOR = 1.05

# (gate id, comparator, threshold source, value source). Fixed order = report order.
GATES = (
    ("cost_aware_alpha_pct", ">=", ("benchmark", "min_alpha_pct"), "benchmark_evidence.alpha_pct"),
    ("total_return_pct", ">=", SCANNER_MIN_TOTAL_RETURN_PCT, "rust_total_return_pct"),
    ("sharpe", ">=", ("promotion_policy", "MQK_PROMOTION_MIN_SHARPE"), "sharpe"),
    ("dsr", ">=", ("promotion_policy", "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"), "dsr"),
    ("pbo", "<=", ("promotion_policy", "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"), "judge.pbo"),
    ("cagr", ">=", ("promotion_policy", "MQK_PROMOTION_MIN_CAGR"), "cagr"),
    ("max_drawdown_pct", "<=", SCANNER_MAX_DRAWDOWN_PCT, "rust_max_drawdown_pct"),
    ("profit_factor", ">=", ("promotion_policy", "MQK_PROMOTION_MIN_PROFIT_FACTOR"), "rust_profit_factor"),
    ("profitable_months", ">=", ("promotion_policy", "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT"), "profitable_months"),
    ("trade_count", ">=", SCANNER_MIN_TRADES, "rust_trade_count"),
)
# Sharpe / CAGR / profitable months are read from the Research walk-forward aggregate: a screening proxy for the
# canonical Promotion metric, which is computed from the Backtest equity curve by `evaluate_promotion`.
PROXY_SOURCES = {"sharpe": "economic_walk_forward_aggregate", "cagr": "economic_walk_forward_aggregate",
                 "profitable_months": "economic_walk_forward_daily_returns"}
EXECUTION_FIDELITY_GATE = "execution_fidelity"

CAVEATS = ("EXPOSED_DEVELOPMENT_NOT_INDEPENDENT", "CUMULATIVE_SEARCH_NOT_DEFLATED")


def _num(x):
    """A finite float, else None (unavailable fails closed)."""
    if isinstance(x, bool) or x is None:
        return None
    try:
        v = float(x)
    except (TypeError, ValueError):
        return None
    return v if math.isfinite(v) else None


def _threshold(decl: dict, ref):
    if isinstance(ref, tuple):
        return _num(decl[ref[0]][ref[1]])
    return float(ref)


def _value(row: dict, judge: dict, source: str):
    if source == "benchmark_evidence.alpha_pct":
        return _num((row.get("benchmark_evidence") or {}).get("alpha_pct"))
    if source == "judge.pbo":
        pbo = judge.get("pbo_result") or {}
        return _num(pbo.get("pbo")) if pbo.get("status") == "evaluated" else None
    return _num(row.get(source))


def gate_result(gate_id: str, comparator: str, threshold: float, value) -> dict:
    """One quantitative gate with its exact signed distance. Equality passes (matches the evaluators'
    strict `<` / `>` failure tests). An unavailable value fails closed with no distance."""
    base = {"gate": gate_id, "comparator": comparator, "threshold": threshold, "value": value}
    if value is None:
        return {**base, "status": "NOT_AVAILABLE", "passed": False, "margin": None, "shortfall": None,
                "relative_shortfall": None}
    margin = value - threshold if comparator == ">=" else threshold - value
    shortfall = max(0.0, -margin)
    rel = None if threshold == 0 else shortfall / abs(threshold)
    return {**base, "status": "PASS" if shortfall == 0 else "FAIL", "passed": shortfall == 0, "margin": margin,
            "shortfall": shortfall, "relative_shortfall": rel}


def _row_index(rows: list[dict]) -> tuple[dict, list[str], list[str]]:
    by: dict[str, list[dict]] = {}
    for r in rows:
        by.setdefault(f"{r.get('strategy')}/{r.get('symbol')}", []).append(r)
    duplicates = sorted(k for k, v in by.items() if len(v) > 1)
    return by, duplicates, []


def review_trial(slot: dict, rows_for_slot: list[dict], decl: dict, judge: dict, verdicts: dict | None,
                 provenance: dict | None, fingerprints: dict) -> dict:
    strategy, symbol = slot["strategy_id"], slot["symbol"]
    out = {"order": slot["order"], "strategy_id": strategy, "symbol": symbol,
           "hypothesis_label": slot["hypothesis_label"], "trial_id": None, "semantic_fingerprint": None,
           "evidence_grade": decl["evidence_grade"]["grade"],
           "data_partition": {"evaluation_start_utc": decl["partition"]["evaluation_start_utc"],
                              "test_months": decl["partition"]["test_months"],
                              "holdout": decl["holdout"]["status"]},
           "evaluability": None, "gates": [], "execution_fidelity": None, "structural": {}, "failed_gates": [],
           "reason_codes": [], "caveats": list(CAVEATS), "final_status": None}
    reasons = out["reason_codes"]
    if not rows_for_slot:
        out["evaluability"] = "MISSING_EVIDENCE"
        reasons.append("MISSING_EVIDENCE_ROW")
        out["final_status"] = UNQUALIFIED
        return out
    if len(rows_for_slot) > 1:
        out["evaluability"] = "DUPLICATE_EVIDENCE"
        reasons.append("DUPLICATE_EVIDENCE_ROWS")
        out["final_status"] = INVALID
        return out
    row = rows_for_slot[0]
    out["trial_id"] = row.get("trial_id")
    out["semantic_fingerprint"] = row.get("semantic_fingerprint")
    pinned = fingerprints.get(symbol)
    if pinned is not None and row.get("semantic_fingerprint") not in (None, pinned):
        reasons.append("IDENTITY_FINGERPRINT_DIFFERS_FROM_DECLARATION")
    if not row.get("trial_id"):
        reasons.append("IDENTITY_TRIAL_ID_MISSING")
    if reasons:
        out["evaluability"] = "IDENTITY_INVALID"
        out["final_status"] = INVALID
        return out
    if row.get("economic_failed"):
        out["evaluability"] = "ECONOMIC_ATTEMPT_FAILED"
        reasons.append("ECONOMIC_ATTEMPT_FAILED")
        out["final_status"] = UNQUALIFIED
        return out
    judge_state = str(row.get("judge_status"))
    out["structural"]["judge_inclusion"] = judge_state
    if judge_state != "included":
        out["evaluability"] = "JUDGE_EXCLUDED"
        reasons.append(f"JUDGE_{judge_state.upper().replace(':', '_')}")
        out["final_status"] = UNQUALIFIED
        return out
    out["evaluability"] = "EVALUABLE"

    fidelity_floor = float(decl["execution_fidelity"]["floor"])
    fidelity = _num(row.get("position_agreement"))
    out["execution_fidelity"] = gate_result(EXECUTION_FIDELITY_GATE, ">=", fidelity_floor, fidelity)

    gates = []
    for gate_id, cmp_, thr_ref, src in GATES:
        g = gate_result(gate_id, cmp_, _threshold(decl, thr_ref), _value(row, judge, src))
        if gate_id in PROXY_SOURCES:
            g["value_source"] = PROXY_SOURCES[gate_id]
            g["canonical_metric"] = "evaluate_promotion on the canonical Backtest equity curve"
        gates.append(g)
    out["gates"] = gates
    out["failed_gates"] = [g["gate"] for g in gates if not g["passed"]]

    # structural evidence
    robustness_failed = list(row.get("robustness_failed") or [])
    robustness_missing = list(row.get("robustness_missing") or [])
    stress_failed = list(row.get("stress_failed") or [])
    out["structural"].update({
        "robustness_failed": robustness_failed, "robustness_missing": robustness_missing,
        "robustness_not_applicable": list(row.get("robustness_not_applicable") or []),
        "stress_failed": stress_failed, "review_state": row.get("review_state"),
        "review_reason_codes": row.get("reason_codes"),
        "judge_status": judge.get("judge_status"), "pbo_status": (judge.get("pbo_result") or {}).get("status")})

    # safety / identity evidence: class INVALID
    unavailable = [g["gate"] for g in gates if g["status"] == "NOT_AVAILABLE" and g["gate"] not in ("dsr", "pbo")]
    reasons.extend(f"REQUIRED_METRIC_UNAVAILABLE:{g}" for g in unavailable)
    if robustness_missing:
        reasons.append("REQUIRED_ROBUSTNESS_EVIDENCE_MISSING")
    if fidelity is None or not out["execution_fidelity"]["passed"]:
        reasons.append("EXECUTION_FIDELITY_BELOW_FLOOR_OR_UNAVAILABLE")
    review_reasons = str(row.get("reason_codes") or "")
    if "halted" in review_reasons.split(";"):
        reasons.append("BACKTEST_HALTED")
    scanner_gate_ids = {"total_return_pct", "cost_aware_alpha_pct", "max_drawdown_pct", "trade_count", "profit_factor"}
    scanner_expected_pass = all(g["passed"] for g in gates if g["gate"] in scanner_gate_ids)
    if scanner_expected_pass != (row.get("review_state") == "paper_candidate"):
        reasons.append("REVIEW_STATE_INCONSISTENT_WITH_ITS_OWN_METRICS")
    if reasons:
        out["final_status"] = INVALID
        return out

    # structural qualification: class UNQUALIFIED
    if judge.get("judge_status") != "evaluated" or (judge.get("pbo_result") or {}).get("status") != "evaluated" \
            or row.get("dsr") is None:
        reasons.append("JUDGE_DSR_PBO_NOT_EVALUABLE")
    if robustness_failed:
        reasons.append("ROBUSTNESS_FAILED:" + ",".join(robustness_failed))
    if stress_failed:
        reasons.append("NATIVE_STRESS_FAILED:" + ",".join(stress_failed))
    if reasons:
        out["final_status"] = UNQUALIFIED
        return out

    if out["failed_gates"]:
        reasons.extend(f"QUANTITATIVE_GATE_FAILED:{g}" for g in out["failed_gates"])
        out["final_status"] = NEAR_MISS
        return out

    # every gate passed: provenance and the canonical verdict decide the last step
    prov = provenance or {}
    missing = [k for k in ("bars_provenance_manifest_present", "holdout_guard_post_passed") if prov.get(k) is not True]
    reasons.extend(f"PROVENANCE_UNVERIFIED:{k}" for k in missing)
    verdict = (verdicts or {}).get(row.get("trial_id"))
    out["structural"]["canonical_promotion_verdict"] = None if verdict is None else bool(verdict.get("passed"))
    if verdict is None:
        reasons.append("CANONICAL_PROMOTION_VERDICT_MISSING")
    elif verdict.get("passed") is not True:
        reasons.append("CANONICAL_PROMOTION_VERDICT_NEGATIVE")
    if reasons:
        out["final_status"] = INSUFFICIENT
        return out
    reasons.append("ALL_GATES_PASSED")
    out["final_status"] = QUALIFIES
    return out


def build_review(decl: dict, rows: list[dict], judge: dict, *, verdicts: dict | None = None,
                 provenance: dict | None = None, accounting_count: int | None = None) -> dict:
    """The full report. Deterministic: ordered by the declaration's trial order, no timestamps, no ranking."""
    slots = decl["universe"]["trials"]
    fingerprints = {sym: fp["capital_fraction_wrapped_semantic_fingerprint"]
                    for sym, fp in decl.get("strategy_fingerprints", {}).get("per_symbol", {}).items()}
    by, duplicates, _ = _row_index(rows)
    declared = {f"{s['strategy_id']}/{s['symbol']}" for s in slots}
    unexpected = sorted(set(by) - declared)
    trials = [review_trial(s, by.get(f"{s['strategy_id']}/{s['symbol']}", []), decl, judge, verdicts, provenance,
                           fingerprints) for s in slots]
    counts = {name: sum(1 for t in trials if t["final_status"] == name) for name in STATUSES}
    population_ok = not unexpected and not duplicates and all(t["evaluability"] != "MISSING_EVIDENCE" for t in trials)
    count = accounting_count if accounting_count is not None else decl["search_accounting"]["cumulative_disclosed_search_count"]
    report = {
        "schema": SCHEMA,
        "campaign_id": decl["campaign_id"],
        "evidence_grade": decl["evidence_grade"]["grade"],
        "independent_confirmation": decl["evidence_grade"]["independent_confirmation"],
        "cumulative_disclosed_search_count": count,
        "judge": {"judge_status": judge.get("judge_status"), "pbo_status": (judge.get("pbo_result") or {}).get("status"),
                  "pbo": (judge.get("pbo_result") or {}).get("pbo"),
                  "registry_population": judge.get("registry_population"),
                  "excluded_trial_ids": judge.get("excluded_trial_ids")},
        "population": {"expected_trials": len(slots), "reviewed_trials": len(trials), "rows_supplied": len(rows),
                       "duplicate_rows": duplicates, "unexpected_rows": unexpected,
                       "status": "COMPLETE" if population_ok else "UNRESOLVED"},
        "status_counts": counts,
        "trials": trials,
        "selection": None,
        "automatic_selection": False,
        "notes": ["Every registered trial is listed; none is selected, ranked or recommended.",
                  "A near-miss is a measurement, not a candidate: a follow-up needs a new prospective declaration "
                  "with a new semantic identity and honest accounting; the exposed window never becomes fresh OOS.",
                  "Sharpe, CAGR and profitable months are Research walk-forward screening values; the canonical "
                  "Promotion metrics come from evaluate_promotion on the Backtest equity curve."],
    }
    return report


def canonical_json(report: dict) -> str:
    return json.dumps(report, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def report_sha256(report: dict) -> str:
    return hashlib.sha256(canonical_json(report).encode("utf-8")).hexdigest()


def render_markdown(report: dict) -> str:
    def f(v, spec=".4f"):
        return "n/a" if v is None else format(v, spec)

    lines = [f"# {report['campaign_id']} results review ({report['schema']})", "",
             f"Evidence grade `{report['evidence_grade']}`; independent confirmation `{report['independent_confirmation']}`; "
             f"cumulative disclosed search count {report['cumulative_disclosed_search_count']} (not deflated by the judge).",
             f"Population: {report['population']['status']} ({report['population']['reviewed_trials']} of "
             f"{report['population']['expected_trials']} registered trials reviewed). No trial is selected.", "",
             "| # | strategy | symbol | evaluability | final status | failed gates | reason codes |", "|---|---|---|---|---|---|---|"]
    for t in report["trials"]:
        lines.append(f"| {t['order']} | {t['strategy_id']} | {t['symbol']} | {t['evaluability']} | {t['final_status']} | "
                     f"{','.join(t['failed_gates']) or '-'} | {';'.join(t['reason_codes'])} |")
    lines += ["", "## Gate distances (value / threshold / signed margin; negative margin = shortfall)", ""]
    for t in report["trials"]:
        lines.append(f"### {t['order']}. {t['strategy_id']} / {t['symbol']} ({t['final_status']})")
        if not t["gates"]:
            lines += ["no gate values (non-evaluable or invalid)", ""]
            continue
        lines += ["| gate | value | cmp | threshold | margin | status |", "|---|---|---|---|---|---|"]
        for g in t["gates"] + ([t["execution_fidelity"]] if t["execution_fidelity"] else []):
            lines.append(f"| {g['gate']} | {f(g['value'])} | {g['comparator']} | {f(g['threshold'])} | "
                         f"{f(g['margin'])} | {g['status']} |")
        lines.append("")
    return "\n".join(lines)


def _provenance_from_run(run: Path) -> dict:
    guard = run / "holdout_guard_post.json"
    ok = False
    if guard.exists():
        try:
            ok = json.loads(guard.read_text(encoding="utf-8")).get("ledger_all_reserved") is True
        except (OSError, ValueError):
            ok = False
    return {"bars_provenance_manifest_present": (run / "data" / "research_bars_provenance.json").exists(),
            "holdout_guard_post_passed": ok}


def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    decl_name = os.environ["MQK_M1_BATCH_DECLARATION"]
    decl = json.loads((HERE / decl_name).read_text(encoding="utf-8"))
    gate = decl.get("execution_gate") or {}
    if gate.get("executable") is not True:
        raise SystemExit(f"fail-closed: {decl['batch_id']} is {gate.get('status')} (blocker {gate.get('blocker')}); "
                         "there is no authorized execution to review")
    run = HERE / decl["run_dir"]
    out_path = None
    if "--out" in argv:
        out_path = Path(argv[argv.index("--out") + 1]).resolve()
        if run.resolve() in out_path.parents:
            raise SystemExit("fail-closed: the review is never written into the run directory it reviews")
    rows = json.loads((run / "batch_results.json").read_text(encoding="utf-8"))
    judge = json.loads((run / "judge" / "judge.json").read_text(encoding="utf-8"))
    promo = run / "promotion_eligibility.json"
    verdicts = json.loads(promo.read_text(encoding="utf-8")) if promo.exists() else None
    report = build_review(decl, rows, judge, verdicts=verdicts, provenance=_provenance_from_run(run))
    text = json.dumps(report, sort_keys=True, indent=1)
    if out_path is not None:
        out_path.write_text(text + "\n", encoding="utf-8")
        out_path.with_suffix(".md").write_text(render_markdown(report) + "\n", encoding="utf-8")
    print(text)


if __name__ == "__main__":
    main()
