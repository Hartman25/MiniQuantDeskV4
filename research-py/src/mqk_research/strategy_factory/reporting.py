"""Campaign report assembly: one deterministic, read-only document per campaign, built only from durable artifacts.

Every section carries a truth state: PRESENT (read from an artifact), EMPTY (the artifact exists and holds nothing) or
UNAVAILABLE (the authority is missing; the reason is stated). Nothing is defaulted, estimated or inferred, and an
UNAVAILABLE section is never rendered as an authoritative empty result. Economic numbers come only from the accepted
batch-results table (`summarize_batch.py`) and registry/judge artifacts; this module computes none of them.
"""

from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any, Mapping

from mqk_research.strategy_factory.campaign import EXPERIMENTS_REL
from mqk_research.strategy_factory.contracts import atomic_write_text
from mqk_research.strategy_factory.contracts import FACTORY_AUTHORITY, promotion_view, sha

REPORT_SCHEMA = "strategy_factory_campaign_report_v1"
PRESENT, EMPTY, UNAVAILABLE = "PRESENT", "EMPTY", "UNAVAILABLE"


def _section(state: str, data: Any = None, reason: str | None = None) -> dict[str, Any]:
    out: dict[str, Any] = {"truth_state": state}
    if data is not None:
        out["data"] = data
    if reason:
        out["reason"] = reason
    return out


def _json(path: Path) -> tuple[str, Any, str | None]:
    if not path.is_file():
        return UNAVAILABLE, None, f"{path.name} not found"
    try:
        return PRESENT, json.loads(path.read_text(encoding="utf-8")), None
    except (OSError, ValueError) as exc:
        return UNAVAILABLE, None, f"{path.name} unreadable: {exc}"


def _file_sha(path: Path) -> str | None:
    if not path.is_file():
        return None
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _registry(db: Path, experiment: str) -> dict[str, Any]:
    if not db.is_file():
        return _section(UNAVAILABLE, reason=f"registry {db.name} does not exist")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    try:
        trials = [dict(r) for r in con.execute(
            "select trial_id, hypothesis_id, strategy_id from research_trials where experiment_id=? order by trial_id", (experiment,))]
        att = [dict(r) for r in con.execute(
            "select a.trial_id, a.attempt_index, a.status, a.failure_reason, a.result_id from research_attempts a "
            "join research_trials t using(trial_id) where t.experiment_id=? order by a.trial_id, a.attempt_index", (experiment,))]
        judges = [dict(r) for r in con.execute(
            "select judge_id, judge_artifact_sha256, schema_version from research_judge_artifacts where experiment_id=? order by judge_artifact_sha256",
            (experiment,))]
        holdout = [dict(r) for r in con.execute("select holdout_id, status, consumed_at from research_holdout_ledger order by holdout_id")]
    except sqlite3.Error as exc:
        return _section(UNAVAILABLE, reason=f"registry unreadable: {exc}")
    finally:
        con.close()
    by_status: dict[str, int] = {}
    for a in att:
        by_status[a["status"]] = by_status.get(a["status"], 0) + 1
    return _section(PRESENT, {"registered_trials": len(trials), "attempts": len(att), "attempts_by_status": dict(sorted(by_status.items())),
                              "retried_trials": sorted({a["trial_id"] for a in att if a["attempt_index"] > 1}),
                              "failed_attempts": [a for a in att if a["status"] == "failed"], "trials": trials,
                              "judge_artifacts": judges, "holdout_ledger": holdout})


def run_summary(decl_path: Path, repo_root: Path) -> tuple[bool, str]:
    """Produce the accepted per-trial evidence table (batch_results.json) from the stage artifacts."""
    exp = Path(repo_root) / EXPERIMENTS_REL
    env = {**os.environ, "MQK_M1_BATCH_DECLARATION": str(decl_path), "PYTHONDONTWRITEBYTECODE": "1"}
    proc = subprocess.run([sys.executable, str(exp / "summarize_batch.py")], cwd=exp, env=env, capture_output=True, text=True, timeout=1800)
    return proc.returncode == 0, (proc.stderr or proc.stdout or "").strip()[-400:]


def build_report(campaign: Mapping[str, Any], repo_root: Path, store: Any = None, *, run_summary_stage: bool = True) -> dict[str, Any]:
    decl_path = Path(campaign["declaration_path"])
    decl = json.loads(decl_path.read_text(encoding="utf-8"))
    run = Path(decl["run_dir"])
    experiment = decl["experiment"]["real_experiment_id"]
    summary_ok, summary_note = (None, "summary stage not requested")
    if run_summary_stage and (run / "trials_index.json").is_file() and (run / "judge" / "judge.json").is_file():
        try:
            summary_ok, summary_note = run_summary(decl_path, repo_root)
        except (subprocess.SubprocessError, OSError) as exc:
            summary_ok, summary_note = False, str(exc)
    idx_state, index, idx_reason = _json(run / "trials_index.json")
    judge_state, judge, judge_reason = _json(run / "judge" / "judge.json")
    res_state, results, res_reason = _json(run / "batch_results.json")
    hg_state, hg, hg_reason = _json(run / "holdout_guard_post.json")
    man_state, manifest, man_reason = _json(run / "data" / "research_bars_provenance.json")
    trials_decl = decl["universe"]["trials"]
    per_trial = []
    for t in trials_decl:
        k = f"{t['strategy_id']}/{t['symbol']}"
        rec = (index or {}).get(k, {}) if idx_state == PRESENT else {}
        if not rec:
            status = "NOT_EXECUTED"
        elif "failed" in rec:
            status = "FAILED_ATTEMPT_KEPT"
        elif "economic_eval_id" in rec:
            status = "EVALUATED"
        else:
            status = "REGISTERED_ONLY"
        row = next((r for r in (results or []) if r.get("strategy") == t["strategy_id"] and r.get("symbol") == t["symbol"]), None) \
            if res_state == PRESENT else None
        per_trial.append({"trial_key": k, "execution_status": status, "trial_id": rec.get("trial_id"),
                          "semantic_fingerprint": rec.get("semantic_fingerprint"), "attempt_index": rec.get("attempt_index"),
                          "failure": rec.get("failed"), "evidence_row": row})
    jobs = store.list_jobs(campaign["campaign_id"]) if store is not None else None
    history = None if jobs is None else [{"stage": j["stage"], "status": j["status"], "attempts": [
        {"attempt_no": a["attempt_no"], "status": a["status"], "reason": a["reason"], "exit_code": a["exit_code"]}
        for a in store.attempts(j["job_id"])]} for j in jobs]
    hy = {h["strategy_id"]: h for h in decl["hypotheses"]}
    counts: dict[str, int] = {}
    for p in per_trial:
        counts[p["execution_status"]] = counts.get(p["execution_status"], 0) + 1
    review_states: dict[str, int] = {}
    for p in per_trial:
        r = p["evidence_row"]
        if r:
            review_states[r.get("review_state", "UNKNOWN")] = review_states.get(r.get("review_state", "UNKNOWN"), 0) + 1
    report = {
        "schema": REPORT_SCHEMA,
        "campaign": {"campaign_id": campaign["campaign_id"], "state": campaign["state"], "evidence_grade": decl["evidence_grade"]["grade"],
                     "evidence_statement": decl["evidence_grade"]["statement"], **promotion_view(decl["evidence_grade"]["grade"]),
                     "declaration_sha256": campaign["declaration_sha256"], "spec_sha256": decl["factory"]["spec_sha256"],
                     "protocol_profile": decl["factory"]["protocol_profile"], "protocol_profile_pin": decl["factory"]["protocol_profile_pin"]},
        "idea_sources_and_hypotheses": _section(PRESENT, [
            {**s, "required_history_bars": hy[s["strategy_name"]]["required_history_bars"],
             "hypothesis_id": hy[s["strategy_name"]]["hypothesis_id"]} for s in decl["factory"]["strategies"]]),
        "population_declaration": _section(PRESENT, {
            "trial_count": len(trials_decl), "unique_strategies": len(decl["factory"]["strategies"]), "symbols": decl["universe"]["symbols"],
            "excluded_invalid_combinations": decl["factory"]["population_report"]["excluded"],
            "max_trials": decl["universe"]["max_trials"], "additional_candidates_after_predeclaration": "forbidden",
            "prior_search_disclosure": decl["factory"]["prior_search_disclosure"]}),
        "registry": _registry(Path(decl["experiment"]["registry_db_relative_path"]), experiment),
        "trial_execution": _section(PRESENT if per_trial else EMPTY, {"status_counts": dict(sorted(counts.items())), "trials": per_trial}),
        "economic_and_benchmark_results": _section(res_state, results, res_reason) if res_state == PRESENT else
            _section(UNAVAILABLE, reason=res_reason or f"batch_results.json not produced ({summary_note})"),
        "cost_model": _section(PRESENT, {"cost_model": decl["economic_protocol"]["cost_model"], "execution_pricing": decl["economic_protocol"]["execution_pricing"],
                                         "capital_sizing": decl.get("capital_sizing"), "benchmark": decl["benchmark"]}),
        "data_identity": _section(man_state, {k: manifest.get(k) for k in ("artifact_sha256", "canonical_semantic_bars_hash", "row_count",
                                                                              "source_attestation_id", "start_utc", "end_utc", "timeframe")}, man_reason)
        if man_state == PRESENT else _section(UNAVAILABLE, reason=man_reason),
        "statistical_judge": _section(PRESENT, {"judge_status": judge["judge_status"], "registry_population": judge["registry_population"],
                                                "included": len(judge["included_trial_ids"]), "excluded": judge["excluded_trial_ids"],
                                                "pbo_result": judge["pbo_result"], "dsr_trial_accounting": judge["dsr_trial_accounting"]})
        if judge_state == PRESENT else _section(UNAVAILABLE, reason=judge_reason),
        "candidate_review": _section(PRESENT if review_states else UNAVAILABLE, review_states or None,
                                     None if review_states else "no scanner review rows yet"),
        "oos_and_holdout": _section(hg_state, hg, hg_reason),
        "artifacts": {name: {"path": str(p).replace("\\", "/"), "sha256": _file_sha(p)} for name, p in (
            ("declaration", decl_path), ("trials_index", run / "trials_index.json"), ("judge", run / "judge" / "judge.json"),
            ("batch_results", run / "batch_results.json"), ("holdout_guard_post", run / "holdout_guard_post.json"),
            ("registry", Path(decl["experiment"]["registry_db_relative_path"])))},
        "restart_and_retry_history": _section(PRESENT if history else UNAVAILABLE, history,
                                              None if history else "no job store attached to this report"),
        "unknown_or_unsupported_requirements": _section(PRESENT, decl["factory"]["population_report"]["excluded"] or []),
        "authority": {**FACTORY_AUTHORITY, "holdout": "RESERVED_NOT_CONSUMED_BY_THIS_CAMPAIGN"},
        "summary_stage": {"ok": summary_ok, "note": summary_note},
    }
    report["report_sha256"] = sha({k: v for k, v in report.items() if k != "restart_and_retry_history"})
    return report


def write_campaign_report(campaign: Mapping[str, Any], repo_root: Path, store: Any = None) -> Path:
    report = build_report(campaign, repo_root, store)
    out = Path(json.loads(Path(campaign["declaration_path"]).read_text(encoding="utf-8"))["run_dir"]) / "factory_report"
    out.mkdir(parents=True, exist_ok=True)
    atomic_write_text(out / "report.json", json.dumps(report, indent=1, sort_keys=True))
    c = report["campaign"]
    lines = [f"# Campaign {c['campaign_id']} — {c['state']}", "", f"Evidence grade: **{c['evidence_grade']}** — {c['evidence_statement']}", "",
             f"Declaration `{c['declaration_sha256'][:16]}…` · profile `{c['protocol_profile']}` · promotion readiness: {c['promotion_readiness']}", ""]
    for k, v in report.items():
        if isinstance(v, dict) and "truth_state" in v:
            lines.append(f"- **{k}**: {v['truth_state']}" + (f" — {v['reason']}" if v.get("reason") else ""))
    lines += ["", f"Trials: {json.dumps(report['trial_execution']['data']['status_counts'])}", "",
              "Authority: " + "; ".join(f"{k}={v}" for k, v in report["authority"].items())]
    atomic_write_text(out / "report.md", "\n".join(lines) + "\n")
    return out / "report.json"
