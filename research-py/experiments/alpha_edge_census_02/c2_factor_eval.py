"""Census-02 short ConditionalEdge evaluator. A factor is one semantic condition x horizon over the frozen eligible universe.
The accepted Census-01 statistics are reused unchanged (values bound in the structural protocol): per-symbol demeaned
forward-return label, registered diagnostics, two-sided empirical null, complete-family BH/FDR. `fwd_ret` is a label and
never executable P&L; direction is lower_is_better and never flips after a result."""

from __future__ import annotations

import contextlib
import json
import sys
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_factors as fx  # noqa: E402
import c2_grammar as gr  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_signals as sg2  # noqa: E402
import conditional as cd1  # noqa: E402  (accepted Census-01 label/null/record helpers, reused)
import edge_registry as er  # noqa: E402
import signals as sg1  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.factors.contracts import (  # noqa: E402
    EVAL_STATUS_NOT_EVALUABLE, EVAL_STATUS_SUCCEEDED, FactorEvaluationSpec)
from mqk_research.factors.diagnostics import FactorDiagnosticsProtocolSpec  # noqa: E402
from mqk_research.factors.fdr import FactorPValueEvidence, build_fdr_population_report  # noqa: E402
from mqk_research.factors.registry import list_factor_evaluation_attempts, list_factors, register_factor  # noqa: E402
from mqk_research.factors.runner import UNIVERSE_MODE_FIXED_EX_ANTE, run_registered_factor_diagnostics  # noqa: E402

STATS = pr.CONDITIONAL_STATISTICS
ORIGIN = "alpha_census02_conditional"
FREEZE_PREFIX = f"{fx.FACTOR_FAMILY}:FACTOR_FREEZE:"
FREEZE_EXPERIMENT_ID = pr.EXPERIMENT_ID + ":conditional"
REUSE_STATUSES = (EVAL_STATUS_SUCCEEDED, EVAL_STATUS_NOT_EVALUABLE)
INTERRUPTED_REASON = cd1.INTERRUPTED_REASON


class FactorPopulationRefusal(RuntimeError):
    pass


class FactorResumeRefusal(RuntimeError):
    pass


class Universe2(sg1.Universe):
    """Census-01 universe with the Census-02 fence applied before any array is built and Census-02 signal dispatch."""

    def __init__(self, symbol_bars: dict):
        self.symbols = sorted(symbol_bars)
        self.sd = {s: sg2.build_symbol_data(s, b) for s, b in symbol_bars.items()}
        self.G = len(sg1.SESSIONS)
        self._assign_regime()

    def build(self, symbol: str, family: str, params: dict):
        return sg2.build(self.sd[symbol], family, params)


# ----------------------------------------------------------------------------------------------- identities / population
def population_context(universe: dict, bars_manifest: dict, protocol_id: str) -> dict:
    """Universe + data-provenance identities every factor binds. Exists only after authorised acquisition."""
    universe_identity = {"universe_id": pr.sha256_canonical(universe)[:32], "symbols_sha256": pr.sha256_canonical(universe["symbols"]),
                         "symbol_count": universe["symbol_count"], "scope": "ALL_SEED_SYMBOLS",
                         "survivorship_classification": universe["survivorship_classification"], "universe_mode": "fixed_ex_ante"}
    provenance = {"bars_manifest_sha256": bars_manifest["manifest_sha256"],
                  "request_contract_sha256": pr.sha256_canonical(pr.DATA_REQUEST_CONTRACT),
                  "partitions_id": pop.partitions_id(), "protocol_id": protocol_id}
    return {"universe_identity": universe_identity, "data_provenance_identity": provenance}


def declared_universe_identity(ctx: dict) -> dict:
    return {k: v for k, v in ctx["universe_identity"].items() if k != "universe_mode"}


def materialize_specs(decisions: dict, ctx: dict, frozen_factor_population: dict) -> list:
    """All frozen FactorSpecs, materialised against the actual provenance BEFORE any evaluation. Exact parity with the frozen
    semantic coordinates (count, coordinate set, direction) is required; nothing is created lazily or for winners only."""
    conds = pop.conditions(decisions)
    items = list(fx.iter_factor_specs(conds, ctx))
    coords = [(c["condition_id"], h) for c, h, _s in items]
    ids = [s.compute_factor_id() for _c, _h, s in items]
    if len(items) != frozen_factor_population["factor_count"] or len(set(ids)) != len(ids) or \
            pop.sorted_root(f"{c}:{h}" for c, h in coords) != frozen_factor_population["coordinate_root"]:
        raise FactorPopulationRefusal("materialised factor population differs from the frozen semantic coordinates")
    if {s.direction for _c, _h, s in items} != {fx.SHORT_FACTOR_DIRECTION}:
        raise FactorPopulationRefusal("every Census-02 short factor must be lower_is_better")
    return items


def factor_population_root(items) -> str:
    return pr.sha256_canonical(sorted(s.compute_factor_id() for _c, _h, s in items))


def register_factor_population(registry_db: Path, items) -> dict:
    """Register EVERY frozen FactorSpec, then the freeze marker. Must complete (with zero attempts) before attempt #1."""
    for _c, _h, spec in items:
        register_factor(Path(registry_db), spec)
    marker = {"family": fx.FACTOR_FAMILY, "factor_count": len(items), "population_root": factor_population_root(items)}
    ResearchResultStore(Path(registry_db)).register_hypothesis(
        hypothesis_id=FREEZE_PREFIX + marker["population_root"], experiment_id=FREEZE_EXPERIMENT_ID,
        hypothesis_text=json.dumps(marker, sort_keys=True, separators=(",", ":")))
    return marker


def require_registered_factor_population(registry_db: Path, items, *, allow_attempts: bool) -> dict:
    """Refuse unless the registry holds exactly the expected factor population and its freeze marker (and, before attempt #1,
    zero attempts). A partially / lazily registered population can never be evaluated."""
    expected = sorted(s.compute_factor_id() for _c, _h, s in items)
    reg = sorted(f["factor_id"] for f in list_factors(Path(registry_db), family=fx.FACTOR_FAMILY))
    if reg != expected:
        raise FactorPopulationRefusal(f"registered factors != frozen population: missing={len(set(expected) - set(reg))} "
                                      f"extra={len(set(reg) - set(expected))}")
    root = factor_population_root(items)
    with contextlib.closing(ResearchResultStore(Path(registry_db))._connect()) as con:  # noqa: SLF001 - read-only probe
        row = con.execute("select hypothesis_text from research_hypotheses where hypothesis_id=?", (FREEZE_PREFIX + root,)).fetchone()
    if row is None or json.loads(row[0]) != {"family": fx.FACTOR_FAMILY, "factor_count": len(items), "population_root": root}:
        raise FactorPopulationRefusal("factor freeze marker absent or different; evaluation before complete registration refused")
    attempts = sum(len(list_factor_evaluation_attempts(Path(registry_db), f)) for f in expected)
    if attempts and not allow_attempts:
        raise FactorPopulationRefusal("factor attempts already exist; the pre-evaluation check demands zero")
    return {"registered": len(reg), "attempts": attempts, "population_root": root}


# ---------------------------------------------------------------------------------------------------- observations frame
def condition_signal(U, symbol: str, condition: dict):
    fam = condition["family"]
    return U.build(symbol, fam, {**condition["params"], **gr.EXECUTION_ONLY_FILL.get(fam, {})})


def build_frame(U, condition: dict, horizon: int) -> tuple[pd.DataFrame, pd.DataFrame]:
    """One row per symbol bar t with the condition defined and t+horizon available. factor_value = completed-bar condition
    (0/1); label = close_{t+h}/close_t - 1 minus the same symbol's same-horizon unconditional mean over the same rows."""
    parts = []
    for sym in U.symbols:
        sd = U.sd[sym]
        sig = condition_signal(U, sym, condition)
        if sig is None:
            continue
        idx = np.arange(sig.s, sd.n - horizon)
        if idx.size == 0:
            continue
        ret = sd.c[idx + horizon] / sd.c[idx] - 1.0
        parts.append(pd.DataFrame({
            "symbol": sym, "period_ts_utc": sd.end_iso[idx], "factor_value": sig.cond[idx].astype(np.float64),
            "label_fwd_ret": ret - ret.mean(), "information_cutoff_ts_utc": sd.end_iso[idx],
            "label_end_ts_utc": sd.end_iso[idx + horizon], "year": sd.years[idx], "regime": sd.regime[idx]}))
    if not parts:
        empty = pd.DataFrame({c: [] for c in cd1.FRAME_COLUMNS})
        return empty, pd.DataFrame({"year": [], "regime": []})
    df = pd.concat(parts, ignore_index=True).sort_values(["period_ts_utc", "symbol"], kind="mergesort").reset_index(drop=True)
    return df[cd1.FRAME_COLUMNS].copy(), df[["year", "regime"]].copy()


# --------------------------------------------------------------------------------------------- registered evaluation
def _protocol_spec(direction: str) -> FactorDiagnosticsProtocolSpec:
    return FactorDiagnosticsProtocolSpec(direction=direction, n_quantiles=STATS["n_quantiles"],
                                         min_cross_section=STATS["min_cross_section"], min_periods=STATS["min_periods"])


def expected_evaluation_id(spec) -> str:
    return FactorEvaluationSpec(
        factor_id=spec.compute_factor_id(),
        universe_identity={**declared_universe_identity({"universe_identity": spec.universe_identity}),
                           "universe_mode": UNIVERSE_MODE_FIXED_EX_ANTE},
        evaluation_window_start_utc=STATS["window_start_utc"], evaluation_window_end_utc=STATS["window_end_utc"],
        label_protocol_version=STATS["label_protocol_version"],
        evaluation_protocol_version=_protocol_spec(spec.direction).evaluation_protocol_version()).compute_evaluation_id()


def _evidence(rec: dict, artifact: dict, frame, aux, spec, cache) -> dict:
    metrics = artifact["metrics"]
    return {**rec, "observations_content_sha256": artifact["input_provenance"]["content_sha256"], "mean_ic": metrics["mean_ic"],
            "period_count": metrics["period_count"], "top_minus_bottom_spread": metrics["quantile"]["top_minus_bottom_spread"],
            "events": cd1.event_diagnostics(frame, aux, spec.direction),
            "pvalue": cd1.fast_empirical_pvalue(frame, metrics, n_permutations=STATS["empirical_null"]["n_permutations"],
                                                base_seed=STATS["empirical_null"]["base_seed"], cache=cache)}


def evaluate_factor(registry_db: Path, out_dir: Path, U, condition: dict, horizon: int, spec, *, cache=None,
                    metadata: dict | None = None) -> dict:
    """One registered evaluation attempt of one frozen factor (the attempt is opened by the registered runner before any
    computation). The evidence record never feeds back into identity."""
    if spec.direction != fx.SHORT_FACTOR_DIRECTION:
        raise fx.FactorAuthorityRefusal("short factor direction must be lower_is_better")
    frame, aux = build_frame(U, condition, horizon)
    res = run_registered_factor_diagnostics(
        Path(registry_db), Path(out_dir), factor_spec=spec, observations=frame,
        evaluation_window_start_utc=STATS["window_start_utc"], evaluation_window_end_utc=STATS["window_end_utc"],
        label_protocol_version=STATS["label_protocol_version"],
        universe_identity=declared_universe_identity({"universe_identity": spec.universe_identity}),
        n_quantiles=STATS["n_quantiles"], min_cross_section=STATS["min_cross_section"], min_periods=STATS["min_periods"],
        holdout_status="discovery_only_no_holdout_consumed", origin=ORIGIN, metadata=metadata)
    rec = {"factor_id": res.factor_id, "evaluation_id": res.evaluation_id, "attempt_id": res.attempt_id,
           "attempt_index": res.attempt_index, "condition_id": condition["condition_id"], "family": condition["family"],
           "horizon": horizon, "status": res.status, "reason": res.reason, "direction": spec.direction,
           "executable_pnl": False}
    if res.status != EVAL_STATUS_SUCCEEDED:
        return rec
    return _evidence(rec, json.loads(Path(res.artifact_path).read_text(encoding="utf-8")), frame, aux, spec, cache)


def _reconstruct(U, condition, horizon, spec, att, cache) -> dict:
    base = {"factor_id": att["factor_id"], "evaluation_id": att["evaluation_id"], "attempt_id": att["attempt_id"],
            "attempt_index": att["attempt_index"], "condition_id": condition["condition_id"], "family": condition["family"],
            "horizon": horizon, "status": att["status"], "reason": att.get("failure_reason"), "direction": spec.direction,
            "executable_pnl": False}
    if att["status"] != EVAL_STATUS_SUCCEEDED:
        return base
    path = (att.get("artifact_paths") or {}).get("factor_diagnostics")
    if not path or not Path(path).exists():
        raise FactorResumeRefusal(f"{att['attempt_id']}: succeeded but its diagnostics artifact is unavailable; refusing to "
                                  "fabricate evidence or re-attempt a terminal factor")
    artifact = json.loads(Path(path).read_text(encoding="utf-8"))
    frame, aux = build_frame(U, condition, horizon)
    from mqk_research.factors.diagnostics import observations_content_hash
    if artifact.get("factor_id") != att["factor_id"] or artifact.get("evaluation_id") != att["evaluation_id"] or \
            observations_content_hash(frame) != artifact["input_provenance"]["content_sha256"]:
        raise FactorResumeRefusal(f"{att['attempt_id']}: rebuilt observations do not match the registered artifact")
    return _evidence(base, artifact, frame, aux, spec, cache)


def resolve_factor(registry_db: Path, out_dir: Path, rec_dir: Path, U, condition: dict, horizon: int, spec, *, cache=None,
                   metadata: dict | None = None, evaluate=None) -> tuple[dict, str]:
    """(terminal record, action) for ONE registered factor; a terminal factor is never re-attempted. started -> finalized
    `infrastructure_interrupted` then retried as a NEW ATTEMPT of the same factor; any other failure reason is refused."""
    evaluate = evaluate or evaluate_factor
    fid = spec.compute_factor_id()
    store = ResearchResultStore(Path(registry_db))
    try:
        store.get_factor(fid)
    except KeyError:
        raise FactorResumeRefusal(f"factor {fid} is not registered in the frozen population; refusing to attempt") from None
    attempts = store.list_factor_evaluation_attempts(fid)
    eid = expected_evaluation_id(spec)
    for att in attempts:
        if (att["status"] == "started" or cd1.retry_eligible(att)) and att.get("evaluation_id") != eid:
            raise FactorResumeRefusal(f"{att['attempt_id']}: attempt belongs to a foreign evaluation; refusing to resume")
    for att in attempts:
        if att["status"] == "started":
            store.finalize_factor_evaluation_attempt(att["attempt_id"], status="failed", expected_factor_id=fid,
                                                     expected_evaluation_id=eid, failure_reason=INTERRUPTED_REASON)
    attempts = store.list_factor_evaluation_attempts(fid)
    latest = attempts[-1] if attempts else None
    if latest is not None and latest["status"] in REUSE_STATUSES:
        rec = cd1.read_factor_record(rec_dir, fid)
        if not cd1._record_binds(rec, fid, latest):  # noqa: SLF001
            rec = _reconstruct(U, condition, horizon, spec, latest, cache)
            cd1.write_factor_record(rec_dir, rec)
            return rec, "reconstructed"
        return rec, "reused"
    if any(a["status"] in REUSE_STATUSES for a in attempts):
        raise FactorResumeRefusal(f"{fid}: a terminal attempt exists but the latest attempt is {latest['status']}; authority conflict")
    if latest is not None and not cd1.retry_eligible(latest):
        raise FactorResumeRefusal(f"{latest['attempt_id']}: failed with a reason other than {INTERRUPTED_REASON!r}; not retried")
    rec = evaluate(registry_db, out_dir, U, condition, horizon, spec, cache=cache, metadata=metadata)
    cd1.write_factor_record(rec_dir, rec)
    return rec, "retried" if latest is not None else "evaluated"


def run_factors(registry_db: Path, out_dir: Path, rec_dir: Path, U, items, *, max_factors: int | None = None, log=print) -> dict:
    """Resumable factor execution in population order. The COMPLETE frozen population must already be registered."""
    require_registered_factor_population(registry_db, items, allow_attempts=True)
    cache, done, ran = cd1.PermutationCache(), 0, 0
    for cond, h, spec in items:
        if max_factors is not None and ran >= max_factors:
            break
        _rec, action = resolve_factor(registry_db, out_dir, rec_dir, U, cond, h, spec, cache=cache)
        done += 1
        ran += action in ("evaluated", "retried")
    log(f"factors processed {done}/{len(items)} (evaluated this call: {ran})")
    return {"factors_total": len(items), "processed": done, "evaluated_this_call": ran}


def load_factor_records(registry_db: Path, rec_dir: Path, items) -> list[dict]:
    """Every frozen factor's registry-bound record in population order; refuses while any factor is unsettled."""
    store = ResearchResultStore(Path(registry_db))
    out = []
    for cond, h, spec in items:
        fid = spec.compute_factor_id()
        attempts = store.list_factor_evaluation_attempts(fid)
        rec = cd1.read_factor_record(rec_dir, fid)
        if not attempts or attempts[-1]["status"] not in REUSE_STATUSES or not cd1._record_binds(rec, fid, attempts[-1]):  # noqa: SLF001
            raise FactorResumeRefusal(f"{cond['family']} {cond['condition_id']} h{h} is not settled in the registry")
        out.append(rec)
    return out


# ------------------------------------------------------------------------------------------------- FDR / classification
def family_fdr_report(registry_db: Path, records: list[dict]) -> dict:
    """BH/FDR over the FULL registered Census-02 factor family; failed / non-evaluable factors stay accounted."""
    items = [FactorPValueEvidence(factor_id=e["factor_id"], evaluation_id=e["evaluation_id"], p_value=e["pvalue"]["p_value"])
             for e in records if e.get("pvalue")]
    return build_fdr_population_report(Path(registry_db), family=fx.FACTOR_FAMILY, p_value_evidence=items,
                                       alpha=STATS["fdr"]["alpha"])


def conditional_edge_records(items, records: list[dict], fdr: dict) -> list[dict]:
    """ConditionalEdge records (highest class only) from the accepted Census-01 classification rule. The FDR population must
    be exactly the registered population (never a winners subset)."""
    expected = sorted(s.compute_factor_id() for _c, _h, s in items)
    auth = er.authoritative_factor_records(records)
    if sorted(auth) != expected or sorted(fdr["declared_factor_ids"]) != expected:
        raise FactorPopulationRefusal("factor records / FDR population differ from the registered factor population")
    out = []
    for fid in expected:
        cls = er.conditional_class(auth[fid], fdr)
        if cls:
            out.append({"edge_id": pr.sha256_canonical({"kind": "conditional_edge", "factor_id": fid})[:32], "factor_id": fid,
                        "class": cls, "direction": fx.SHORT_FACTOR_DIRECTION, "VALIDATION_STATUS": "NOT_VALIDATED",
                        "PROMOTION_AUTHORITY": "NONE", "executable_pnl": False})
    return out
