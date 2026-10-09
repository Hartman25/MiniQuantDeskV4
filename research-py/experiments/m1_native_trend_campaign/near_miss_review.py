"""Read-only, deterministic results review for a graded M1 campaign (`m1_near_miss_review_v2`).

Prepared BEFORE any economic execution; populated only after authorized execution. It reviews EVERY slot
of the declaration (failures, non-evaluable attempts and missing evidence included), in the declaration's
trial order, reports each authoritative gate with its unit, comparator, threshold, evidence source and exact
signed distance, and selects nothing: no rank, no winner, no "best near-miss".

Pure over its inputs. It reads no market data, opens no registry itself (the CLI loads a read-only registry
extract and passes it in), writes nothing into the run directory and makes no network call.

Identity is never taken from the evidence row's say-so. The expected trial id of every slot is RECOMPUTED
from the declaration, the declared native fingerprint and the data provenance manifest; a row, a registry
record and a judge entry must each agree with it exactly.

Final status per trial, mutually exclusive, first match wins:

  1 INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE      identity differs from the declared/recomputed one, duplicate
                                                       row, judge/registry/attempt contradiction, invalid numeric
                                                       evidence, fidelity below floor, halted/blocked backtest,
                                                       required evidence missing, inconsistent review state
  2 STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE          missing row, failed attempt, judge could not include the
                                                       trial, DSR/PBO not evaluable, a robustness/stress scenario failed
  3 INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION  identity/data/holdout provenance unverified, or every research
                                                       gate passes but no valid canonical Promotion proof exists
  4 QUANTITATIVE_ECONOMIC_NEAR_MISS                    evaluable, valid, provenanced; at least one gate fails by number
  5 QUALIFICATION_WITHHELD                             every gate passes on this trial's own evidence but a global
                                                       condition (population, accounting, cumulative-search
                                                       validation, holdout incident) blocks any qualification
  6 QUALIFIES_UNDER_EVERY_APPLICABLE_GATE              every gate and every global condition holds

Provenance (3) is decided BEFORE the numbers (4): missing provenance is never presented as a quantitative
near-miss. `QUALIFIES` is not independent confirmation: the evidence grade is EXPOSED_DEVELOPMENT.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import sqlite3
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import holdout_incident  # noqa: E402
import search_accounting  # noqa: E402

SCHEMA = "m1_near_miss_review_v2"

INVALID = "INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE"
UNQUALIFIED = "STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE"
INSUFFICIENT = "INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION"
NEAR_MISS = "QUANTITATIVE_ECONOMIC_NEAR_MISS"
WITHHELD = "QUALIFICATION_WITHHELD"
QUALIFIES = "QUALIFIES_UNDER_EVERY_APPLICABLE_GATE"
STATUSES = (QUALIFIES, WITHHELD, NEAR_MISS, INSUFFICIENT, UNQUALIFIED, INVALID)

# mqk_backtest::StrategyScanReviewPolicy::default (pinned to the Rust source by test).
SCANNER_MIN_BARS_USED = 252
SCANNER_MIN_TRADES = 5
SCANNER_MIN_TOTAL_RETURN_PCT = 0.0
SCANNER_MAX_DRAWDOWN_PCT = 25.0
SCANNER_MIN_PROFIT_FACTOR = 1.05
EXECUTION_FIDELITY_GATE = "execution_fidelity"

PROMO = "promotion_policy"
# (gate id, comparator, threshold, unit, authority, evidence source, domain(lo, hi, integer),
#  evidence field, canonical field | None, proxy field)
# Threshold is a literal (scanner policy) or ("decl", block, key). Equality passes everywhere: the Rust
# evaluators fail only on strict `<` / `>`.
GATES = (
    ("scanner_min_bars_used", ">=", SCANNER_MIN_BARS_USED, "bars", "scanner", "scan candidates.json metrics.bars_used",
     (0, None, True), "rust_bars_used", None),
    ("scanner_min_trade_count", ">=", SCANNER_MIN_TRADES, "trades", "scanner", "backtest metrics.json trade_count",
     (0, None, True), "rust_trade_count", None),
    ("scanner_min_total_return_pct", ">=", SCANNER_MIN_TOTAL_RETURN_PCT, "percent", "scanner",
     "backtest metrics.json total_return_pct", (-100.0, None, False), "rust_total_return_pct", None),
    ("scanner_min_alpha_pct", ">=", ("decl", "benchmark", "min_alpha_pct"), "percent", "scanner",
     "scan review benchmark evidence alpha_pct", (None, None, False), "benchmark_evidence.alpha_pct", None),
    ("scanner_max_drawdown_pct", "<=", SCANNER_MAX_DRAWDOWN_PCT, "percent", "scanner",
     "backtest metrics.json max_drawdown_pct", (0.0, 100.0, False), "rust_max_drawdown_pct", None),
    ("scanner_min_profit_factor", ">=", SCANNER_MIN_PROFIT_FACTOR, "ratio", "scanner",
     "backtest metrics.json profit_factor", (0.0, None, False), "rust_profit_factor", None),
    ("judge_min_dsr", ">=", ("decl", PROMO, "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"), "probability", "judge",
     "judge dsr_results[trial].deflated_sharpe_ratio", (0.0, 1.0, False), "dsr", None),
    ("judge_max_pbo", "<=", ("decl", PROMO, "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"), "probability",
     "judge", "judge pbo_result.pbo", (0.0, 1.0, False), "judge.pbo", None),
    ("promotion_min_sharpe", ">=", ("decl", PROMO, "MQK_PROMOTION_MIN_SHARPE"), "annualized ratio", "promotion",
     "evaluate_promotion metrics.sharpe", (None, None, False), "sharpe", "sharpe"),
    ("promotion_max_drawdown", "<=", ("decl", PROMO, "MQK_PROMOTION_MAX_MDD"), "fraction", "promotion",
     "evaluate_promotion metrics.mdd", (0.0, 1.0, False), "max_drawdown", "mdd"),
    ("promotion_min_cagr", ">=", ("decl", PROMO, "MQK_PROMOTION_MIN_CAGR"), "fraction", "promotion",
     "evaluate_promotion metrics.cagr", (-1.0, None, False), "cagr", "cagr"),
    ("promotion_min_profit_factor", ">=", ("decl", PROMO, "MQK_PROMOTION_MIN_PROFIT_FACTOR"), "ratio", "promotion",
     "evaluate_promotion metrics.profit_factor", (0.0, None, False), "rust_profit_factor", "profit_factor"),
    ("promotion_min_profitable_months_pct", ">=", ("decl", PROMO, "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT"),
     "fraction", "promotion", "evaluate_promotion metrics.profitable_months_pct", (0.0, 1.0, False),
     "profitable_months", "profitable_months_pct"),
)
SCANNER_GATE_IDS = tuple(g[0] for g in GATES if g[4] == "scanner")
CANONICAL_GATE_IDS = tuple(g[0] for g in GATES if g[8] is not None)

PROMOTION_CONFIG_KEYS = {  # PromotionConfig field -> declaration key
    "min_sharpe": "MQK_PROMOTION_MIN_SHARPE", "max_mdd": "MQK_PROMOTION_MAX_MDD", "min_cagr": "MQK_PROMOTION_MIN_CAGR",
    "min_profit_factor": "MQK_PROMOTION_MIN_PROFIT_FACTOR",
    "min_profitable_months_pct": "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT",
    "min_deflated_sharpe_ratio": "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO",
    "max_probability_backtest_overfitting": "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"}

CAVEATS = ("EXPOSED_DEVELOPMENT_NOT_INDEPENDENT", "CUMULATIVE_SEARCH_NOT_DEFLATED_BY_THE_FOUR_TRIAL_JUDGE")


# ------------------------------------------------------------------ strict numeric evidence

def strict_number(x, *, lo=None, hi=None, integer: bool = False):
    """(value, None) for a real JSON number inside its domain, else (None, reason). Booleans, strings,
    NaN/Infinity and unsupported types are never numbers; a missing value is MISSING, not zero."""
    if x is None:
        return None, "MISSING"
    if isinstance(x, bool) or not isinstance(x, (int, float)):
        return None, "INVALID_TYPE"
    if integer and not isinstance(x, int):
        return None, "INVALID_TYPE"
    if isinstance(x, float) and not math.isfinite(x):
        return None, "NON_FINITE"
    if (lo is not None and x < lo) or (hi is not None and x > hi):
        return None, "OUT_OF_RANGE"
    return (int(x) if integer else float(x)), None


def _path(row: dict, dotted: str):
    cur = row
    for part in dotted.split("."):
        if not isinstance(cur, dict):
            return None
        cur = cur.get(part)
    return cur


def _threshold(decl: dict, ref) -> float:
    if isinstance(ref, tuple):
        value, err = strict_number(decl[ref[1]][ref[2]])
        if err:
            raise ValueError(f"declared threshold {ref[1]}.{ref[2]} is not a number ({err})")
        return value
    return float(ref)


def gate_result(gate_id: str, comparator: str, threshold: float, value, *, unit: str = "", authority: str = "",
                source: str = "", basis: str = "DIRECT") -> dict:
    """One gate with its exact signed distance. Equality passes. An unavailable value fails closed with no
    distance. A PROXY value is shown with its distance but can never satisfy a canonical gate."""
    base = {"gate": gate_id, "comparator": comparator, "threshold": threshold, "unit": unit, "authority": authority,
            "evidence_source": source, "basis": basis, "value": value}
    if value is None:
        return {**base, "status": "NOT_AVAILABLE", "passed": False, "margin": None, "shortfall": None,
                "relative_shortfall": None}
    margin = value - threshold if comparator == ">=" else threshold - value
    shortfall = max(0.0, -margin)
    rel = None if threshold == 0 else shortfall / abs(threshold)
    ok = shortfall == 0
    if basis == "PROXY":
        status = "PROXY_ONLY_PASS" if ok else "PROXY_FAIL"
    else:
        status = "PASS" if ok else "FAIL"
    return {**base, "status": status, "passed": ok and basis != "PROXY", "margin": margin, "shortfall": shortfall,
            "relative_shortfall": rel}


# ------------------------------------------------------------------ canonical Promotion proof

class ProofInvalid(ValueError):
    """The supplied Promotion report is not a valid, run-bound canonical report."""


class PromotionProof:
    """A canonical `PromotionReport` that has been validated against the declared policy and bound to one
    Backtest run. Only `from_report` constructs it; a free-form mapping such as {"trial": {"passed": true}}
    is not a proof. The report is unsigned JSON produced by the Rust evaluator: the binding is to the run, the
    strategy and the declared thresholds, and that limit is stated in every report that uses one."""

    _TOKEN = object()
    __slots__ = ("run_id", "strategy", "passed", "metrics", "source")

    def __init__(self, token, run_id, strategy, passed, metrics, source):
        if token is not PromotionProof._TOKEN:
            raise TypeError("PromotionProof is constructed only by PromotionProof.from_report")
        self.run_id, self.strategy, self.passed, self.metrics, self.source = run_id, strategy, passed, metrics, source

    @classmethod
    def from_report(cls, report, *, run_id: str, strategy: str, policy: dict, source: str = "") -> "PromotionProof":
        if not isinstance(report, dict):
            raise ProofInvalid("report is not an object")
        prov, cfg, decision = report.get("provenance"), report.get("config"), report.get("decision")
        metrics = report.get("metrics")
        if not all(isinstance(x, dict) for x in (prov, cfg, decision, metrics)):
            raise ProofInvalid("report lacks provenance/config/metrics/decision objects")
        rid = str(prov.get("run_id", "")).lower()
        if not rid or set(rid) <= {"0", "-"}:
            raise ProofInvalid("provenance.run_id is missing or nil")
        if rid != str(run_id).lower():
            raise ProofInvalid("provenance.run_id differs from the Backtest run this trial was evaluated on")
        if prov.get("strategy_name") != strategy:
            raise ProofInvalid("provenance.strategy_name differs from the trial's strategy")
        if set(cfg) != set(PROMOTION_CONFIG_KEYS):
            raise ProofInvalid("config does not have exactly the PromotionConfig fields")
        for field, key in PROMOTION_CONFIG_KEYS.items():
            if cfg[field] != policy[key] or isinstance(cfg[field], bool):
                raise ProofInvalid(f"config.{field} differs from the declared {key}")
        if decision.get("metrics") != metrics:
            raise ProofInvalid("decision.metrics differs from metrics")
        passed, reasons = decision.get("passed"), decision.get("fail_reasons")
        if not isinstance(passed, bool) or not isinstance(reasons, list):
            raise ProofInvalid("decision.passed must be a boolean and fail_reasons a list")
        if passed != (len(reasons) == 0):
            raise ProofInvalid("decision.passed contradicts decision.fail_reasons")
        if metrics.get("execution_blocked") is not False:
            raise ProofInvalid("the Backtest run was execution-blocked or the flag is not boolean false")
        values = {}
        for field in ("sharpe", "mdd", "cagr", "profit_factor", "profitable_months_pct"):
            v, err = strict_number(metrics.get(field))
            if err:
                raise ProofInvalid(f"metrics.{field} is {err}")
            values[field] = v
        if passed:
            ok = (values["sharpe"] >= cfg["min_sharpe"] and values["mdd"] <= cfg["max_mdd"]
                  and values["cagr"] >= cfg["min_cagr"] and values["profit_factor"] >= cfg["min_profit_factor"]
                  and values["profitable_months_pct"] >= cfg["min_profitable_months_pct"])
            if not ok:
                raise ProofInvalid("decision.passed is true but the reported metrics do not meet the config thresholds")
        return cls(cls._TOKEN, rid, strategy, passed, values, source)


def load_promotion_proofs(run: Path, rows: list[dict], decl: dict) -> tuple[dict, dict]:
    """(proofs by trial_id, rejections by trial_id) from the canonical location
    `run/promotion/<strategy>/<symbol>/promotion_report.json`, each cross-checked against the Backtest run's
    own manifest. No producer writes these files today; absence is reported, never filled in."""
    proofs, problems = {}, {}
    for row in rows:
        trial_id = row.get("trial_id")
        if not trial_id or row.get("economic_failed"):
            continue
        path = run / "promotion" / str(row.get("strategy")) / str(row.get("symbol")) / "promotion_report.json"
        if not path.is_file():
            continue
        try:
            run_id = row.get("backtest_run_id")
            manifest_path = run / "backtest" / str(row["strategy"]) / str(row["symbol"]) / str(run_id) / "manifest.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if str(manifest.get("run_id")).lower() != str(run_id).lower() or manifest.get("strategy_name") != row["strategy"]:
                raise ProofInvalid("the Backtest manifest does not carry the run id/strategy this trial recorded")
            proofs[trial_id] = PromotionProof.from_report(
                json.loads(path.read_text(encoding="utf-8")), run_id=str(run_id), strategy=row["strategy"],
                policy=decl[PROMO], source=str(path.relative_to(run)))
        except (OSError, ValueError, KeyError, TypeError) as exc:
            problems[trial_id] = f"{type(exc).__name__}: {exc}"
    return proofs, problems


# ------------------------------------------------------------------ registry, judge, provenance, accounting

def _canonical_identity(identity: dict) -> str:
    return json.dumps(identity, sort_keys=True, separators=(",", ":"))


def expected_slot_identities(decl: dict, manifest: dict) -> dict:
    """slot key -> (trial_id, identity) recomputed from the declaration, its declared (capital-fraction wrapped)
    fingerprints and the data provenance manifest -- the same authority the registration gate uses."""
    import run_batch  # the single trial-identity authority; its own declaration load is not used here
    fingerprints = {}
    hyp = {h["strategy_id"]: h for h in decl["hypotheses"]}
    for t in decl["universe"]["trials"]:
        fp = decl["strategy_fingerprints"]["per_symbol"][t["symbol"]]["capital_fraction_wrapped_semantic_fingerprint"]
        fingerprints[(t["strategy_id"], t["symbol"])] = (fp, hyp[t["strategy_id"]]["required_history_bars"])
    return {f"{s}/{y}": (tid, ident) for s, y, tid, ident in run_batch.expected_trial_ids(fingerprints, manifest, decl)}


def verify_registry(decl: dict, registry: dict | None, expected: dict | None) -> dict:
    """The registry must hold EXACTLY the recomputed identities (same ids, same canonical identity text, same
    strategy column), with no extra or duplicate record. UNAVAILABLE when either side is missing."""
    if registry is None or expected is None:
        return {"status": "UNAVAILABLE", "problems": ["registry extract or data provenance manifest not supplied"],
                "per_trial": {}}
    problems, per_trial = [], {}
    rows = registry.get("trials") or []
    ids = [r.get("trial_id") for r in rows]
    if len(set(ids)) != len(ids):
        problems.append("DUPLICATE_REGISTRY_TRIAL_ID")
    expected_ids = {tid: (slot, ident) for slot, (tid, ident) in expected.items()}
    for r in rows:
        if r.get("trial_id") not in expected_ids:
            problems.append(f"UNEXPECTED_REGISTRY_TRIAL:{r.get('trial_id')}")
    by_id = {r.get("trial_id"): r for r in rows}
    for slot, (tid, ident) in expected.items():
        rec = by_id.get(tid)
        if rec is None:
            per_trial[slot] = "MISSING_FROM_REGISTRY"
            problems.append(f"MISSING_REGISTRY_TRIAL:{slot}")
        elif rec.get("strategy_id") != slot.split("/")[0] or rec.get("identity_json") != _canonical_identity(ident):
            per_trial[slot] = "IDENTITY_DIFFERS_FROM_RECOMPUTED"
            problems.append(f"REGISTRY_IDENTITY_MISMATCH:{slot}")
        else:
            per_trial[slot] = "MATCH"
    return {"status": "VERIFIED" if not problems else "MISMATCH", "problems": sorted(problems), "per_trial": per_trial}


def verify_judge(decl: dict, judge: dict, expected: dict | None) -> dict:
    """The judge's included and excluded inventories must partition EXACTLY the recomputed trial ids."""
    problems = []
    if (judge.get("scope") or {}).get("experiment_id") != decl["experiment"]["real_experiment_id"] \
            or (judge.get("scope") or {}).get("hypothesis_id") is not None:
        problems.append("JUDGE_SCOPE_IS_NOT_THE_WHOLE_EXPERIMENT")
    if (judge.get("protocol") or {}).get("protocol_id") != decl["judge_plan"]["protocol_id"]:
        problems.append("JUDGE_PROTOCOL_DIFFERS")
    if (judge.get("holdout") or {}).get("status") != "reserved_not_evaluated":
        problems.append("JUDGE_HOLDOUT_STATUS_NOT_RESERVED")
    included = judge.get("included_trial_ids")
    excluded_rows = judge.get("excluded_trial_ids")
    if not isinstance(included, list) or not isinstance(excluded_rows, list) \
            or not all(isinstance(r, dict) and "trial_id" in r and "reason" in r for r in excluded_rows):
        return {"status": "MISMATCH", "problems": ["JUDGE_INVENTORY_MALFORMED"] + problems, "included": [], "excluded": {}}
    excluded = {r["trial_id"]: r["reason"] for r in excluded_rows}
    if len(set(included)) != len(included) or len(excluded) != len(excluded_rows):
        problems.append("JUDGE_INVENTORY_HAS_DUPLICATES")
    if set(included) & set(excluded):
        problems.append("JUDGE_TRIAL_BOTH_INCLUDED_AND_EXCLUDED")
    population = (judge.get("registry_population") or {}).get("registered_unique_trials")
    if population != len(decl["universe"]["trials"]):
        problems.append(f"JUDGE_REGISTRY_POPULATION_{population!r}_NOT_{len(decl['universe']['trials'])}")
    dsr_ids = [r.get("trial_id") for r in judge.get("dsr_results") or []]
    if not set(dsr_ids) <= set(included) or len(set(dsr_ids)) != len(dsr_ids):
        problems.append("JUDGE_DSR_RESULTS_OUTSIDE_THE_INCLUDED_SET")
    if expected is None:
        problems.append("JUDGE_INVENTORY_UNVERIFIABLE_WITHOUT_RECOMPUTED_IDENTITIES")
    else:
        want = {tid for tid, _ in expected.values()}
        have = set(included) | set(excluded)
        if have != want:
            problems.append(f"JUDGE_INVENTORY_DIFFERS_FROM_EXPECTED(missing {len(want - have)}, unexpected {len(have - want)})")
    return {"status": "VERIFIED" if not problems else "MISMATCH", "problems": sorted(problems),
            "included": sorted(included), "excluded": dict(sorted(excluded.items()))}


def verify_provenance(decl: dict, manifest: dict | None, guard: dict | None) -> dict:
    """Data manifest within the declared development window and a complete post-run holdout guard."""
    problems = []
    import pandas as pd
    boundary = (decl.get("partition") or {}).get("holdout_boundary")
    start = pd.Timestamp(boundary["holdout_start_utc"]) if boundary else None
    if not isinstance(manifest, dict) or not manifest:
        problems.append("BARS_PROVENANCE_MANIFEST_MISSING")
    else:
        try:
            if sorted(manifest.get("symbol_universe") or []) != sorted(decl["universe"]["symbols"]):
                problems.append("MANIFEST_SYMBOL_UNIVERSE_DIFFERS")
            if start is not None and pd.Timestamp(manifest["end_utc"]) > start:
                problems.append("MANIFEST_RANGE_REACHES_THE_RESERVED_WINDOW")
            if pd.Timestamp(manifest["start_utc"]) < pd.Timestamp(decl["data"]["start_utc"]):
                problems.append("MANIFEST_RANGE_STARTS_BEFORE_THE_DECLARED_WINDOW")
        except (KeyError, TypeError, ValueError):
            problems.append("MANIFEST_RANGE_UNREADABLE")
    if not isinstance(guard, dict) or not guard:
        problems.append("HOLDOUT_GUARD_POST_MISSING")
    else:
        if guard.get("phase") != "post" or guard.get("ledger_all_reserved") is not True:
            problems.append("HOLDOUT_GUARD_POST_NOT_PASSED")
        if start is not None and guard.get("holdout_start_utc") != start.isoformat():
            problems.append("HOLDOUT_GUARD_BOUNDARY_DIFFERS_FROM_THE_DECLARED_ONE")
        import holdout_guard
        need = {name for name, *_ in holdout_guard.CATEGORIES} | ({holdout_guard.FIXED_PARTITION_CATEGORY[0]} if boundary else set())
        cats = guard.get("categories") or {}
        for name in sorted(need):
            c = cats.get(name)
            if not isinstance(c, dict) or not isinstance(c.get("files"), int) or c["files"] < 1:
                problems.append(f"HOLDOUT_GUARD_CATEGORY_UNCHECKED:{name}")
    return {"status": "VERIFIED" if not problems else "UNVERIFIED", "problems": sorted(problems)}


def verify_accounting(decl: dict) -> dict:
    """The disclosed count is recomputed from the frozen declarations; the declaration's own block must agree."""
    try:
        acc = search_accounting.build_accounting(decl)
    except (ValueError, OSError, KeyError) as exc:
        return {"status": "INVALID", "problems": [f"{type(exc).__name__}: {exc}"], "count": None,
                "cumulative_search_status": search_accounting.CUMULATIVE_SEARCH_VALIDATION_BLOCKED}
    return {"status": "VALID", "problems": [], "count": acc["counts"]["cumulative_disclosed_search_count"],
            "sensitivity_counts": acc["counts"]["cumulative"],
            "cumulative_search_status": acc["statistical_validation"]["cumulative_search_status"],
            "statistical_acceptance_blocker": acc["statistical_validation"]["statistical_acceptance_blocker"]}


# ------------------------------------------------------------------ one trial

def _judge_state(trial_id, judge_check: dict) -> str | None:
    if trial_id in judge_check["included"]:
        return "included"
    if trial_id in judge_check["excluded"]:
        return "excluded:" + str(judge_check["excluded"][trial_id])
    return None


def _row_index(rows: list[dict]) -> tuple[dict, list[str]]:
    by: dict[str, list[dict]] = {}
    for r in rows:
        by.setdefault(f"{r.get('strategy')}/{r.get('symbol')}", []).append(r)
    return by, sorted(k for k, v in by.items() if len(v) > 1)


def review_trial(slot: dict, rows_for_slot: list[dict], ctx: dict) -> dict:
    decl, judge, judge_check = ctx["decl"], ctx["judge"], ctx["judge_check"]
    strategy, symbol = slot["strategy_id"], slot["symbol"]
    key = f"{strategy}/{symbol}"
    expected = ctx["expected"].get(key) if ctx["expected"] else None
    out = {"order": slot["order"], "strategy_id": strategy, "symbol": symbol, "hypothesis_label": slot["hypothesis_label"],
           "trial_id": None, "semantic_fingerprint": None, "expected_trial_id": expected[0] if expected else None,
           "evidence_grade": decl["evidence_grade"]["grade"],
           "data_partition": {"evaluation_start_utc": decl["partition"]["evaluation_start_utc"],
                              "test_months": decl["partition"]["test_months"], "holdout": decl["holdout"]["status"]},
           "evaluability": None, "gates": [], "execution_fidelity": None, "structural": {}, "failed_gates": [],
           "unproven_gates": [], "reason_codes": [], "caveats": list(CAVEATS),
           "canonical_promotion": {"status": "PROMOTION_PROOF_UNAVAILABLE", "passed": None,
                                   "authentication": None, "source": None},
           "withheld_by": [], "final_status": None}
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
    trial_id = row.get("trial_id")
    out["trial_id"], out["semantic_fingerprint"] = trial_id, row.get("semantic_fingerprint")

    # ---- 1. identity (INVALID): the row must be the predeclared trial, not merely carry some id
    declared_fp = decl["strategy_fingerprints"]["per_symbol"].get(symbol, {}).get("capital_fraction_wrapped_semantic_fingerprint")
    if not isinstance(trial_id, str) or not trial_id:
        reasons.append("IDENTITY_TRIAL_ID_MISSING")
    if row.get("semantic_fingerprint") is None:
        reasons.append("IDENTITY_FINGERPRINT_MISSING")
    elif row.get("semantic_fingerprint") != declared_fp:
        reasons.append("IDENTITY_FINGERPRINT_DIFFERS_FROM_DECLARATION")
    if expected is not None and trial_id != expected[0]:
        reasons.append("IDENTITY_TRIAL_ID_NOT_THE_PREDECLARED_ID")
    rec_status = ctx["registry"]["per_trial"].get(key)
    if rec_status in ("MISSING_FROM_REGISTRY", "IDENTITY_DIFFERS_FROM_RECOMPUTED"):
        reasons.append(f"REGISTRY_{rec_status}")
    if reasons:
        out["evaluability"] = "IDENTITY_INVALID"
        out["final_status"] = INVALID
        return out

    # ---- 2. judge accounting of this trial must agree with the judge artifact itself
    judge_state = _judge_state(trial_id, judge_check)
    out["structural"]["judge_inclusion"] = judge_state
    if ctx["expected"] is not None and judge_state is None:
        reasons.append("JUDGE_DOES_NOT_ACCOUNT_FOR_TRIAL")
    elif judge_state is not None and str(row.get("judge_status")) != judge_state:
        reasons.append("JUDGE_STATUS_ROW_CONTRADICTS_JUDGE_ARTIFACT")

    # ---- 3. attempt status must agree with the registry's attempts
    attempts = ctx["attempts"].get(trial_id)
    if attempts is not None:
        if not attempts:
            reasons.append("NO_REGISTERED_ATTEMPT")
        elif row.get("economic_failed"):
            if attempts[-1].get("status") != "failed":
                reasons.append("ATTEMPT_STATUS_CONTRADICTS_FAILED_EVIDENCE")
        else:
            match = [a for a in attempts if a.get("attempt_index") == row.get("attempt_index")]
            if len(match) != 1 or match[0].get("status") != "succeeded":
                reasons.append("ATTEMPT_STATUS_CONTRADICTS_SUCCEEDED_EVIDENCE")
    if reasons:
        out["evaluability"] = "IDENTITY_INVALID"
        out["final_status"] = INVALID
        return out

    if row.get("economic_failed"):
        out["evaluability"] = "ECONOMIC_ATTEMPT_FAILED"
        reasons.append("ECONOMIC_ATTEMPT_FAILED")
        out["final_status"] = UNQUALIFIED
        return out
    if judge_state is not None and judge_state != "included":
        out["evaluability"] = "JUDGE_EXCLUDED"
        reasons.append(f"JUDGE_{judge_state.upper().replace(':', '_')}")
        out["final_status"] = UNQUALIFIED
        return out
    out["evaluability"] = "EVALUABLE"

    # ---- 4. numeric evidence: strict types and domains
    floor = float(decl["execution_fidelity"]["floor"])
    fidelity, ferr = strict_number(row.get("position_agreement"), lo=0.0, hi=1.0)
    out["execution_fidelity"] = gate_result(EXECUTION_FIDELITY_GATE, ">=", floor, fidelity, unit="fraction",
                                            authority="campaign", source="registry execution_fidelity")
    values: dict[str, tuple] = {}
    pbo_block = judge.get("pbo_result") or {}
    judge_dsr = {r.get("trial_id"): r.get("deflated_sharpe_ratio") for r in judge.get("dsr_results") or []}
    for gid, _cmp, _thr, _unit, authority, _src, (lo, hi, integer), field, _canon in GATES:
        if field == "judge.pbo":
            raw = pbo_block.get("pbo") if pbo_block.get("status") == "evaluated" else None
        elif field == "dsr":
            raw = judge_dsr.get(trial_id)
            if "dsr" in row and row["dsr"] != raw:
                reasons.append("DSR_ROW_DIFFERS_FROM_JUDGE")
        else:
            raw = _path(row, field)
        v, err = strict_number(raw, lo=lo, hi=hi, integer=integer)
        values[gid] = (v, err)
        if err in ("INVALID_TYPE", "NON_FINITE", "OUT_OF_RANGE"):
            reasons.append(f"NUMERIC_EVIDENCE_INVALID:{gid}:{err}")
    if ferr in ("INVALID_TYPE", "NON_FINITE", "OUT_OF_RANGE"):
        reasons.append(f"NUMERIC_EVIDENCE_INVALID:{EXECUTION_FIDELITY_GATE}:{ferr}")

    # ---- gates (canonical gates use the validated proof when one exists; the Research value is only a proxy)
    proof = ctx["promotion"].get(trial_id) if ctx["promotion"] else None
    canonical = None
    if isinstance(proof, PromotionProof):
        if proof.run_id != str(row.get("backtest_run_id") or "").lower() or proof.strategy != strategy:
            reasons.append("PROMOTION_PROOF_RUN_MISMATCH")
        else:
            canonical = proof
    elif proof is not None:
        out["canonical_promotion"]["status"] = "PROMOTION_PROOF_UNAVAILABLE"
        out["canonical_promotion"]["rejected"] = "supplied verdict is not a validated canonical PromotionProof"
    elif ctx.get("promotion_problems", {}).get(trial_id):
        out["canonical_promotion"]["rejected"] = ctx["promotion_problems"][trial_id]
    gates = []
    for gid, cmp_, thr, unit, authority, src, _dom, field, canon in GATES:
        v, err = values.get(gid, (None, "MISSING"))
        if canon is not None and canonical is not None:
            cv, cerr = strict_number(canonical.metrics.get(canon))
            gate = gate_result(gid, cmp_, _threshold(decl, thr), cv, unit=unit, authority=authority, source=src,
                               basis="CANONICAL")
            gate["proxy_value"] = v
        elif canon is not None:
            gate = gate_result(gid, cmp_, _threshold(decl, thr), v, unit=unit, authority=authority,
                               source=src + " (canonical; Research proxy shown)", basis="PROXY")
        else:
            gate = gate_result(gid, cmp_, _threshold(decl, thr), v, unit=unit, authority=authority, source=src)
        gates.append(gate)
    out["gates"] = gates
    out["failed_gates"] = [g["gate"] for g in gates if g["status"] in ("FAIL", "PROXY_FAIL")]
    out["unproven_gates"] = [g["gate"] for g in gates if g["status"] in ("NOT_AVAILABLE", "PROXY_ONLY_PASS")]
    if canonical is not None:
        out["canonical_promotion"] = {"status": "VERIFIED_RUN_BOUND_REPORT", "passed": canonical.passed,
                                      "authentication": "RUN_BOUND_UNSIGNED", "source": canonical.source}

    robustness_failed = list(row.get("robustness_failed") or [])
    robustness_missing = list(row.get("robustness_missing") or [])
    stress_failed = list(row.get("stress_failed") or [])
    out["structural"].update({
        "robustness_failed": robustness_failed, "robustness_missing": robustness_missing,
        "robustness_not_applicable": list(row.get("robustness_not_applicable") or []),
        "stress_failed": stress_failed, "review_state": row.get("review_state"),
        "review_reason_codes": row.get("reason_codes"), "judge_status": judge.get("judge_status"),
        "pbo_status": pbo_block.get("status")})

    # ---- 5. safety / required evidence (INVALID)
    for g in gates:
        if g["status"] == "NOT_AVAILABLE" and g["gate"] not in ("judge_min_dsr", "judge_max_pbo") \
                and not (g["authority"] == "promotion" and g["basis"] == "CANONICAL"):
            reasons.append(f"REQUIRED_METRIC_UNAVAILABLE:{g['gate']}")
    ev = row.get("benchmark_evidence")
    if isinstance(ev, dict):  # the alpha is only meaningful against the declared benchmark, for THIS slot and sizing
        if ev.get("policy_id") != decl["benchmark"]["policy_id"]:
            reasons.append("BENCHMARK_EVIDENCE_POLICY_MISMATCH")
        if ev.get("strategy_id") != strategy or ev.get("symbol") != symbol:
            reasons.append("BENCHMARK_EVIDENCE_SLOT_MISMATCH")
        if ev.get("allocation_fraction_bps") != decl["capital_sizing"]["allocation_fraction_bps"]:
            reasons.append("BENCHMARK_EVIDENCE_SIZING_MISMATCH")
    if robustness_missing:
        reasons.append("REQUIRED_ROBUSTNESS_EVIDENCE_MISSING")
    if fidelity is None or not out["execution_fidelity"]["passed"]:
        reasons.append("EXECUTION_FIDELITY_BELOW_FLOOR_OR_UNAVAILABLE")
    if "halted" in str(row.get("reason_codes") or "").split(";"):
        reasons.append("BACKTEST_HALTED")
    scanner_pass = all(g["passed"] for g in gates if g["gate"] in SCANNER_GATE_IDS)
    if scanner_pass != (row.get("review_state") == "paper_candidate"):
        reasons.append("REVIEW_STATE_INCONSISTENT_WITH_ITS_OWN_METRICS")
    if reasons:
        out["final_status"] = INVALID
        return out

    # ---- 6. structural qualification (UNQUALIFIED)
    if judge.get("judge_status") != "evaluated" or pbo_block.get("status") != "evaluated" or values["judge_min_dsr"][0] is None:
        reasons.append("JUDGE_DSR_PBO_NOT_EVALUABLE")
    if robustness_failed:
        reasons.append("ROBUSTNESS_FAILED:" + ",".join(robustness_failed))
    if stress_failed:
        reasons.append("NATIVE_STRESS_FAILED:" + ",".join(stress_failed))
    if reasons:
        out["final_status"] = UNQUALIFIED
        return out

    # ---- 7. provenance BEFORE the numbers: an unprovenanced result is never a "near-miss"
    prov_problems = list(ctx["provenance"]["problems"])
    if ctx["registry"]["status"] == "UNAVAILABLE":
        prov_problems.append("REGISTRY_EVIDENCE_UNAVAILABLE")
    if ctx["expected"] is None:
        prov_problems.append("TRIAL_IDENTITY_NOT_RECOMPUTABLE_WITHOUT_THE_BARS_MANIFEST")
    reasons.extend(f"PROVENANCE_UNVERIFIED:{p}" for p in prov_problems)
    if reasons:
        out["final_status"] = INSUFFICIENT
        return out

    # ---- 8. quantitative failures (NEAR_MISS)
    if out["failed_gates"]:
        reasons.extend(f"QUANTITATIVE_GATE_FAILED:{g}" for g in out["failed_gates"])
        out["final_status"] = NEAR_MISS
        return out

    # ---- 9. every available gate passes: a valid, passing canonical Promotion proof is still required
    if canonical is None:
        reasons.append("PROMOTION_PROOF_UNAVAILABLE")
    elif canonical.passed is not True:
        reasons.append("CANONICAL_PROMOTION_VERDICT_NEGATIVE")
    if out["unproven_gates"] and canonical is not None:
        reasons.extend(f"GATE_UNPROVEN:{g}" for g in out["unproven_gates"])
    if reasons:
        out["final_status"] = INSUFFICIENT
        return out

    # ---- 10. global conditions decide between WITHHELD and QUALIFIES
    if ctx["blockers"]:
        out["withheld_by"] = list(ctx["blockers"])
        reasons.extend(f"WITHHELD_BY:{b}" for b in ctx["blockers"])
        out["final_status"] = WITHHELD
        return out
    reasons.append("ALL_GATES_AND_GLOBAL_CONDITIONS_PASSED")
    out["final_status"] = QUALIFIES
    return out


# ------------------------------------------------------------------ the report

def build_review(decl: dict, rows: list[dict], judge: dict, *, registry: dict | None = None,
                 manifest: dict | None = None, guard: dict | None = None, promotion: dict | None = None,
                 promotion_problems: dict | None = None, incident_entries: list[dict] | None = None) -> dict:
    """The full report. Deterministic: ordered by the declaration's trial order, no timestamps, no ranking.
    There is deliberately no parameter that supplies a search count: it is recomputed from the declarations."""
    slots = decl["universe"]["trials"]
    expected = expected_slot_identities(decl, manifest) if isinstance(manifest, dict) and manifest else None
    registry_check = verify_registry(decl, registry, expected)
    judge_check = verify_judge(decl, judge, expected)
    provenance = verify_provenance(decl, manifest, guard)
    accounting = verify_accounting(decl)
    incidents = holdout_incident.truth_summary(decl, incident_entries)
    by, duplicates = _row_index(rows)
    declared = {f"{s['strategy_id']}/{s['symbol']}" for s in slots}
    unexpected = sorted(set(by) - declared)
    attempts = {}
    if registry is not None:
        for tid, lst in (registry.get("attempts") or {}).items():
            attempts[tid] = sorted(lst, key=lambda a: a.get("attempt_index", 0))
        for r in registry.get("trials") or []:
            attempts.setdefault(r.get("trial_id"), [])

    missing = [k for k in sorted(declared) if k not in by]
    population_ok = not unexpected and not duplicates and not missing
    blockers = []
    if not population_ok:
        blockers.append("POPULATION_UNRESOLVED")
    if judge_check["status"] != "VERIFIED":
        blockers.append("JUDGE_INVENTORY_NOT_VERIFIED")
    if registry_check["status"] != "VERIFIED":
        blockers.append("REGISTRY_NOT_VERIFIED")
    if provenance["status"] != "VERIFIED":
        blockers.append("PROVENANCE_NOT_VERIFIED")
    if accounting["status"] != "VALID":
        blockers.append("ACCOUNTING_INVALID")
    if accounting["cumulative_search_status"] != search_accounting.CUMULATIVE_SEARCH_VALIDATED:
        blockers.append(search_accounting.CUMULATIVE_SEARCH_VALIDATION_BLOCKED)
    if incidents["pending_incident_ids"]:
        blockers.append("HOLDOUT_ACCESS_INCIDENT_PENDING_ADJUDICATION")
    if incidents["consumed_incident_ids"]:  # a consumed window is never untouched independent evidence again
        blockers.append("HOLDOUT_CONSUMED_BY_ADJUDICATION")
    ctx = {"decl": decl, "judge": judge, "judge_check": judge_check, "registry": registry_check, "expected": expected,
           "provenance": provenance, "attempts": attempts, "promotion": promotion or {},
           "promotion_problems": promotion_problems or {}, "blockers": blockers}
    trials = [review_trial(s, by.get(f"{s['strategy_id']}/{s['symbol']}", []), ctx) for s in slots]
    counts = {name: sum(1 for t in trials if t["final_status"] == name) for name in STATUSES}
    report = {
        "schema": SCHEMA,
        "campaign_id": decl["campaign_id"],
        "evidence_grade": decl["evidence_grade"]["grade"],
        "independent_confirmation": decl["evidence_grade"]["independent_confirmation"],
        "cumulative_disclosed_search_count": accounting["count"],
        "statistical_validation": {
            "FOUR_TRIAL_JUDGE_RESULT": {"judge_status": judge.get("judge_status"),
                                        "pbo_status": (judge.get("pbo_result") or {}).get("status"),
                                        "pbo": (judge.get("pbo_result") or {}).get("pbo"),
                                        "deflated_for_prior_campaigns": False},
            "CUMULATIVE_SEARCH_DISCLOSED": accounting["count"],
            "cumulative_search_status": accounting["cumulative_search_status"],
            "statistical_acceptance_blocker": accounting.get("statistical_acceptance_blocker")},
        "judge": {"judge_status": judge.get("judge_status"), "registry_population": judge.get("registry_population"),
                  "included_trial_ids": judge_check["included"], "excluded_trial_ids": judge_check["excluded"],
                  "inventory": {"status": judge_check["status"], "problems": judge_check["problems"]}},
        "registry": {"status": registry_check["status"], "problems": registry_check["problems"]},
        "provenance": provenance,
        "accounting": {"status": accounting["status"], "problems": accounting["problems"],
                       "sensitivity_counts": accounting.get("sensitivity_counts")},
        "holdout": incidents,
        "population": {"expected_trials": len(slots), "reviewed_trials": len(trials), "rows_supplied": len(rows),
                       "duplicate_rows": duplicates, "unexpected_rows": unexpected, "missing_rows": missing,
                       "status": "COMPLETE" if population_ok else "UNRESOLVED"},
        "qualification_blockers": blockers,
        "status_counts": counts,
        "trials": trials,
        "selection": None,
        "automatic_selection": False,
        "notes": ["Every slot is listed; none is selected, ranked or recommended.",
                  "A near-miss is a measurement, not a candidate: a follow-up needs a new prospective declaration "
                  "with a new semantic identity and honest accounting; the exposed window never becomes fresh OOS.",
                  "Gates marked PROXY use Research walk-forward values and can never satisfy a canonical Promotion "
                  "gate; only a validated, run-bound canonical Promotion report can.",
                  "A canonical Promotion report is unsigned JSON from the Rust evaluator bound to the Backtest run; "
                  "it is treated as RUN_BOUND_UNSIGNED evidence."],
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
             f"cumulative disclosed search count {report['cumulative_disclosed_search_count']} "
             f"(`{report['statistical_validation']['cumulative_search_status']}`; not deflated by the four-trial judge).",
             f"Population: {report['population']['status']} ({report['population']['reviewed_trials']} of "
             f"{report['population']['expected_trials']} slots reviewed). Holdout: "
             f"`{report['holdout']['access_incident_status']}`. No trial is selected.",
             f"Qualification blockers: {', '.join(report['qualification_blockers']) or 'none'}", "",
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
        lines += ["| gate | unit | value | cmp | threshold | margin | basis | status |", "|---|---|---|---|---|---|---|---|"]
        for g in t["gates"] + ([t["execution_fidelity"]] if t["execution_fidelity"] else []):
            lines.append(f"| {g['gate']} | {g['unit']} | {f(g['value'])} | {g['comparator']} | {f(g['threshold'])} | "
                         f"{f(g['margin'])} | {g['basis']} | {g['status']} |")
        lines.append("")
    return "\n".join(lines)


# ------------------------------------------------------------------ CLI (read-only)

def load_registry_extract(path: Path, experiment_id: str) -> dict:
    """Read-only extract of the experiment's trials and attempts. Never creates or migrates a database."""
    if not path.is_file():
        raise SystemExit(f"fail-closed: registry {path} does not exist")
    con = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        con.row_factory = sqlite3.Row
        trials = [dict(r) for r in con.execute(
            "select trial_id, experiment_id, hypothesis_id, strategy_id, identity_json from research_trials "
            "where experiment_id=? order by trial_id", (experiment_id,))]
        attempts: dict[str, list] = {}
        for r in con.execute("select trial_id, attempt_index, status from research_attempts order by trial_id, attempt_index"):
            attempts.setdefault(r["trial_id"], []).append({"attempt_index": r["attempt_index"], "status": r["status"]})
    finally:
        con.close()
    return {"trials": trials, "attempts": {t["trial_id"]: attempts.get(t["trial_id"], []) for t in trials}}


def _read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


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
    registry = load_registry_extract(HERE / decl["experiment"]["registry_db_relative_path"],
                                     decl["experiment"]["real_experiment_id"])
    proofs, problems = load_promotion_proofs(run, rows, decl)
    report = build_review(decl, rows, judge, registry=registry,
                          manifest=_read_json(run / "data" / "research_bars_provenance.json"),
                          guard=_read_json(run / "holdout_guard_post.json"), promotion=proofs,
                          promotion_problems=problems)
    text = json.dumps(report, sort_keys=True, indent=1)
    if out_path is not None:
        out_path.write_text(text + "\n", encoding="utf-8")
        out_path.with_suffix(".md").write_text(render_markdown(report) + "\n", encoding="utf-8")
    print(text)


if __name__ == "__main__":
    main()
