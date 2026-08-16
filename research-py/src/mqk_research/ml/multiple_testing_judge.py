from __future__ import annotations

import json
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.hashing import canonical_json, short_hash
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml import multiple_testing_stats as mts
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID
from mqk_research.ml.util_hash import sha256_file

# RESEARCH-MULTIPLE-TESTING-JUDGE-01
#
# Statistically honest multiple-testing / backtest-overfitting judge on top
# of the durable research trial registry (RESEARCH-EXPERIMENT-REGISTRY-01),
# purged walk-forward + reserved holdout (RESEARCH-PURGED-HOLDOUT-01), and
# the causal economic walk-forward evaluator (RESEARCH-ECONOMIC-WALKFORWARD-01
# and its REPAIRs). See module docstrings there for the semantics this module
# depends on and must not redesign:
#   - hypothesis -> trial -> attempt (trial identity is result-independent
#     and immutable; attempts are append-only; a retry is a NEW attempt row
#     under the SAME trial_id, never a new trial).
#   - economic_walk_forward_v1's daily NET OOS return series
#     (eval/economic_daily_returns.csv, "net_daily_return" column) is built
#     ONLY from discovery-fold (never holdout) OOS execution -- this module
#     never reads targets.csv/fwd_ret and never evaluates the reserved
#     holdout region.
#
# This module answers a narrower question than "which trial is best": given
# the trials that were ACTUALLY registered (win or lose, retried or not),
# how much should we trust that the best-looking one isn't just the winner
# of a multiple-comparisons lottery? It implements:
#   - Deflated Sharpe Ratio (DSR), Bailey & Lopez de Prado (2014).
#   - Probability of Backtest Overfitting (PBO) via Combinatorially
#     Symmetric Cross-Validation (CSCV), Bailey, Borwein, Lopez de Prado &
#     Zhu (2017).
# The pure formulas live in mqk_research.ml.multiple_testing_stats; this
# module is the registry-integration/orchestration layer: querying the
# durable trial population, resolving which trials are genuinely comparable,
# loading their discovery-OOS daily return series, and serializing one
# deterministic, auditable judge artifact. It does NOT invent a promotion
# threshold and does NOT touch the reserved holdout.

PROTOCOL_ID = "research_multiple_testing_judge_v1"
SCHEMA_VERSION = "multiple_testing_judge_v1"

_TERMINAL_SUCCESS = "succeeded"


@dataclass(frozen=True)
class JudgeSpec:
    """cscv_target_block_count is a target, not a guarantee: the actual
    block count used is derived deterministically from the number of
    available daily observations (see multiple_testing_stats.
    choose_cscv_block_count) so PBO never silently runs on a degenerate
    partition (e.g. a block with 0-1 observations)."""

    protocol_id: str = PROTOCOL_ID
    cscv_target_block_count: int = 10
    schema_version: str = SCHEMA_VERSION

    def normalized(self) -> "JudgeSpec":
        if self.protocol_id != PROTOCOL_ID:
            raise ValueError(f"unsupported judge protocol_id: {self.protocol_id!r}")
        target = int(self.cscv_target_block_count)
        if target < 4 or target % 2 != 0:
            raise ValueError("cscv_target_block_count must be even and >= 4")
        return replace(self, cscv_target_block_count=target)


def _json_field(raw: Optional[str]) -> Any:
    if raw is None:
        return None
    return json.loads(raw)


# ---------------------------------------------------------------------------
# Comparison scope
# ---------------------------------------------------------------------------
#
# What makes two economic_walk_forward_v1 trials genuinely comparable for
# DSR/PBO is NOT "identical strategy" -- if every candidate had to share the
# same signal policy / cost model / feature set there would never be more
# than one candidate to compare, which defeats the purpose of a multiple-
# testing judge. What DOES have to match, because the methodology directly
# depends on it, is the MEASUREMENT BASIS the return series were produced
# under:
#   - protocol_id (must be economic_walk_forward_v1; nothing else is
#     supported by this evaluator).
#   - the underlying market data the simulation executed against
#     (data_identity.economic_bars_csv content hash) -- comparing Sharpe
#     ratios computed against different price histories is not a multiple-
#     testing question, it's a different experiment.
#   - the walk-forward/discovery-period structure (evaluation_spec: train/
#     test/step months, purge/embargo, holdout_months) -- this determines
#     which calendar dates are even eligible to appear in the OOS series,
#     and CSCV's block partition assumes every candidate column was
#     evaluated over the SAME chronological observations.
#   - the annualization convention (days, risk-free rate) -- DSR's PSR
#     benchmark, skew/kurtosis correction, and CSCV's Sharpe-based ranking
#     all assume a single frequency/rate convention across every column of
#     the T x N matrix.
# Signal policy, cost model, model spec, and feature/label identity are
# deliberately EXCLUDED from the comparison key: those differences are
# exactly what makes two trials distinct STRATEGIES worth comparing.
#
# A second, independent gate is applied after the compat-key grouping: the
# candidate's actual daily-return DATE INDEX must match the scope's chosen
# reference date sequence exactly. Two trials can share an identical
# evaluation_spec yet still produce different realized OOS calendars (e.g.
# a universe/feature change that alters which folds get skipped for
# too-few-rows) -- CSCV's row-position block partition is only meaningful
# if every column truly represents the same chronological observations.
#
# RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01: the comparison key also
# includes the cost model (commission/slippage/diagnostic_zero_cost) and the
# execution/capacity/fold-end policy (capacity_policy, fold_end_policy).
# Two candidates measured under different cost or execution/capacity
# assumptions are not answering the same multiple-testing question even if
# their calendars and annualization convention match -- their Sharpe ratios
# were produced by different measurement processes, not just different
# strategies. entry_threshold/long_only/sizing/max_gross_exposure remain
# DELIBERATELY excluded: those are candidate-differentiating strategy
# choices, exactly what a multiple-testing comparison is supposed to range
# over (see test_changing_strategy_threshold_alone_stays_comparable).


def _comparison_key(identity: Dict[str, Any]) -> Tuple[str, Dict[str, Any]]:
    economic_protocol = identity["economic_protocol"]
    signal_policy = economic_protocol["signal_policy"]
    key_obj = {
        "protocol_id": identity["protocol_id"],
        "economic_bars_sha256": identity["data_identity"]["economic_bars_csv"]["sha256"],
        "evaluation_spec": identity["evaluation_spec"],
        "annualization": economic_protocol["annualization"],
        "cost_model": economic_protocol["cost_model"],
        "execution_capacity_policy": {
            "capacity_policy": signal_policy["capacity_policy"],
            "fold_end_policy": signal_policy["fold_end_policy"],
        },
    }
    return canonical_json(key_obj), key_obj


@dataclass
class _LoadedCandidate:
    trial_id: str
    attempt_id: str
    identity: Dict[str, Any]
    economic_eval_id: str
    economic_walk_forward_json_path: Path
    economic_walk_forward_json_sha256: str
    economic_daily_returns_csv_path: Path
    economic_daily_returns_csv_sha256: str
    dates: Tuple[str, ...]
    net_daily_returns: np.ndarray


def _load_candidate(store: ResearchResultStore, trial: Dict[str, Any]) -> Tuple[Optional[_LoadedCandidate], Optional[str]]:
    """Resolve the single attempt whose economic result feeds the judge for
    this trial (the LATEST succeeded attempt, by attempt_index -- a
    deterministic, documented tie-break; earlier attempts, including any
    failures, remain visible in the registry but never feed this
    computation). Returns (candidate, None) or (None, reason)."""
    attempts = store.list_attempts(trial["trial_id"])
    succeeded = [a for a in attempts if a["status"] == _TERMINAL_SUCCESS and a.get("result_id")]
    if not succeeded:
        return None, "no_successful_attempt"
    attempt = max(succeeded, key=lambda a: int(a["attempt_index"]))

    artifact_paths = _json_field(attempt.get("artifact_paths_json")) or {}
    ewf_path_raw = artifact_paths.get("economic_walk_forward")
    if not ewf_path_raw:
        return None, "missing_economic_artifact_path"
    ewf_path = Path(ewf_path_raw)
    if not ewf_path.exists():
        return None, "missing_economic_artifact_file"

    try:
        economic_out = json.loads(ewf_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None, "unreadable_economic_artifact"

    if economic_out.get("schema_version") != "economic_walk_forward_v1":
        return None, "unexpected_economic_artifact_schema"
    if (economic_out.get("protocol") or {}).get("protocol_id") != ECONOMIC_PROTOCOL_ID:
        return None, "unexpected_economic_artifact_schema"
    if economic_out.get("holdout") != {"status": "reserved_not_evaluated"}:
        return None, "unexpected_economic_artifact_schema"

    daily_record = (economic_out.get("outputs") or {}).get("economic_daily_returns_csv")
    if not daily_record or not daily_record.get("path"):
        return None, "missing_daily_returns_artifact"
    daily_path = Path(daily_record["path"])
    if not daily_path.exists():
        return None, "missing_daily_returns_artifact"
    actual_sha256 = sha256_file(daily_path)
    if daily_record.get("sha256") != actual_sha256:
        return None, "artifact_content_hash_mismatch"

    try:
        daily_df = pd.read_csv(daily_path)
    except (OSError, pd.errors.ParserError):
        return None, "unreadable_daily_returns_csv"
    if "date" not in daily_df.columns or "net_daily_return" not in daily_df.columns:
        return None, "malformed_daily_returns_csv"
    if daily_df.empty:
        return None, "empty_return_series"
    if daily_df["date"].duplicated().any():
        return None, "duplicate_dates_in_return_series"

    daily_df = daily_df.sort_values("date", kind="mergesort").reset_index(drop=True)
    returns = pd.to_numeric(daily_df["net_daily_return"], errors="coerce").to_numpy(dtype=np.float64)
    if not np.all(np.isfinite(returns)):
        return None, "non_finite_daily_returns"

    identity = _json_field(trial["identity_json"])
    try:
        _comparison_key(identity)
    except (KeyError, TypeError):
        return None, "malformed_trial_identity"

    candidate = _LoadedCandidate(
        trial_id=trial["trial_id"],
        attempt_id=attempt["attempt_id"],
        identity=identity,
        economic_eval_id=(economic_out.get("ids") or {}).get("economic_eval_id", ""),
        economic_walk_forward_json_path=ewf_path,
        economic_walk_forward_json_sha256=sha256_file(ewf_path),
        economic_daily_returns_csv_path=daily_path,
        economic_daily_returns_csv_sha256=actual_sha256,
        dates=tuple(daily_df["date"].astype(str).tolist()),
        net_daily_returns=returns,
    )
    return candidate, None


def _resolve_comparison_scope(
    candidates: List[_LoadedCandidate],
) -> Tuple[Optional[Dict[str, Any]], List[_LoadedCandidate], Dict[str, str]]:
    """Deterministically choose ONE comparison scope from the loaded,
    successfully-parsed candidates: the largest compat-key group (ties
    broken by lexicographically smallest canonical-JSON key), then within
    that group the largest shared date-index group (same tie-break).
    Returns (comparison_scope_identity_or_None, admitted_candidates,
    {trial_id: exclusion_reason} for everyone else)."""
    if not candidates:
        return None, [], {}

    by_key: Dict[str, List[_LoadedCandidate]] = {}
    key_objs: Dict[str, Dict[str, Any]] = {}
    for c in candidates:
        key_json, key_obj = _comparison_key(c.identity)
        by_key.setdefault(key_json, []).append(c)
        key_objs[key_json] = key_obj

    best_key = sorted(by_key.keys(), key=lambda k: (-len(by_key[k]), k))[0]
    scope_group = sorted(by_key[best_key], key=lambda c: c.trial_id)
    excluded: Dict[str, str] = {
        c.trial_id: "incompatible_comparison_scope"
        for k, cs in by_key.items() if k != best_key for c in cs
    }

    by_dates: Dict[Tuple[str, ...], List[_LoadedCandidate]] = {}
    for c in scope_group:
        by_dates.setdefault(c.dates, []).append(c)
    best_dates = sorted(by_dates.keys(), key=lambda d: (-len(by_dates[d]), canonical_json(list(d))))[0]
    admitted = sorted(by_dates[best_dates], key=lambda c: c.trial_id)
    for dates, cs in by_dates.items():
        if dates != best_dates:
            for c in cs:
                excluded[c.trial_id] = "return_series_date_misalignment"

    scope_identity = dict(key_objs[best_key])
    scope_identity["observations"] = len(best_dates)
    scope_identity["reference_dates"] = list(best_dates)
    return scope_identity, admitted, excluded


# ---------------------------------------------------------------------------
# Top-level judge
# ---------------------------------------------------------------------------


def build_multiple_testing_judge(
    *,
    experiment_id: str,
    hypothesis_id: Optional[str] = None,
    registry_db: Path,
    spec: Optional[JudgeSpec] = None,
) -> Dict[str, Any]:
    if not experiment_id.strip():
        raise ValueError("experiment_id is required")
    spec = (spec or JudgeSpec()).normalized()
    store = ResearchResultStore(Path(registry_db))

    all_trials = store.list_trials(experiment_id=experiment_id, hypothesis_id=hypothesis_id)
    econ_trials = [t for t in all_trials if t["protocol_id"] == ECONOMIC_PROTOCOL_ID]
    econ_trials.sort(key=lambda t: t["trial_id"])

    registered_unique_trials = len(econ_trials)
    attempt_count = 0
    evaluation_slice_count = 0
    attempted_trial_ids: List[str] = []
    attempts_by_trial: Dict[str, List[Dict[str, Any]]] = {}
    for t in econ_trials:
        attempts = store.list_attempts(t["trial_id"])
        attempts_by_trial[t["trial_id"]] = attempts
        attempt_count += len(attempts)
        if attempts:
            attempted_trial_ids.append(t["trial_id"])
        for a in attempts:
            evaluation_slice_count += len(store.list_attempt_slices(a["attempt_id"]))
    attempted_unique_trials = len(attempted_trial_ids)

    candidates: List[_LoadedCandidate] = []
    load_exclusions: Dict[str, str] = {}
    for t in econ_trials:
        if not attempts_by_trial[t["trial_id"]]:
            load_exclusions[t["trial_id"]] = "never_attempted"
            continue
        candidate, reason = _load_candidate(store, t)
        if candidate is None:
            load_exclusions[t["trial_id"]] = reason or "unresolved"
        else:
            candidates.append(candidate)

    comparison_scope, scope_candidates, scope_exclusions = _resolve_comparison_scope(candidates)

    # Second gate: a structurally-comparable candidate whose OWN return
    # series is statistically degenerate (zero variance / too short) cannot
    # contribute a Sharpe estimate to the correction; excluding it here
    # keeps "economically_evaluable_trials" a SINGLE population used
    # consistently by both DSR and PBO below, rather than three silently
    # different trial counts.
    daily_rf = 0.0
    annualization_days = 252
    if comparison_scope is not None:
        annualization_days = int(comparison_scope["annualization"]["annualization_days"])
        risk_free_rate_annual = float(comparison_scope["annualization"]["risk_free_rate_annual"])
        daily_rf = risk_free_rate_annual / annualization_days

    evaluable_candidates: List[_LoadedCandidate] = []
    per_trial_excess: Dict[str, np.ndarray] = {}
    per_trial_stats: Dict[str, Dict[str, Any]] = {}
    for c in scope_candidates:
        excess = c.net_daily_returns - daily_rf
        stats = mts.compute_trial_sharpe_stats(excess)
        per_trial_stats[c.trial_id] = stats
        per_trial_excess[c.trial_id] = excess
        if stats["evaluable"]:
            evaluable_candidates.append(c)
        else:
            scope_exclusions[c.trial_id] = f"degenerate_returns:{stats['reason']}"

    excluded_trial_ids: List[Dict[str, str]] = []
    for trial_id, reason in {**load_exclusions, **scope_exclusions}.items():
        excluded_trial_ids.append({"trial_id": trial_id, "reason": reason})
    excluded_trial_ids.sort(key=lambda r: r["trial_id"])

    included_trial_ids = sorted(c.trial_id for c in evaluable_candidates)
    economically_evaluable_trials = len(included_trial_ids)

    input_economic_result_ids = sorted(c.economic_eval_id for c in evaluable_candidates)
    input_artifacts = sorted(
        (
            {
                "trial_id": c.trial_id,
                "economic_walk_forward_json_path": str(c.economic_walk_forward_json_path),
                "economic_walk_forward_json_sha256": c.economic_walk_forward_json_sha256,
                "economic_daily_returns_csv_path": str(c.economic_daily_returns_csv_path),
                "economic_daily_returns_csv_sha256": c.economic_daily_returns_csv_sha256,
            }
            for c in evaluable_candidates
        ),
        key=lambda r: r["trial_id"],
    )

    registry_population = {
        "registered_unique_trials": registered_unique_trials,
        "attempted_unique_trials": attempted_unique_trials,
        "economically_evaluable_trials": economically_evaluable_trials,
        "attempt_count": attempt_count,
        "evaluation_slice_count": evaluation_slice_count,
    }

    # judge_id basis: EVERY field here is a structural/registry-state fact
    # (what exists, what got included/excluded and why, what content it
    # hashes to) -- never a DSR/PBO NUMERIC result. Re-running the judge
    # after a code change that alters the statistical output, without
    # altering the registry or which artifacts were selected, must not
    # change judge_id.
    judge_id_basis = {
        "protocol_id": spec.protocol_id,
        "schema_version": spec.schema_version,
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "comparison_scope": comparison_scope,
        "registry_population": registry_population,
        "included_trial_ids": included_trial_ids,
        "excluded_trial_ids": excluded_trial_ids,
        "input_economic_result_ids": input_economic_result_ids,
        "input_artifacts": input_artifacts,
        "holdout_status": "reserved_not_evaluated",
    }
    judge_id = short_hash(judge_id_basis, length=32)

    trial_accounting = _compute_dsr_trial_accounting(evaluable_candidates, per_trial_excess)
    dsr_results = _compute_dsr_results(
        evaluable_candidates, per_trial_excess, per_trial_stats, annualization_days, trial_accounting
    )
    pbo_result = _compute_pbo_result(evaluable_candidates, per_trial_excess, spec)

    dsr_evaluable = economically_evaluable_trials >= 2 and any(r["evaluable"] for r in dsr_results)
    pbo_evaluable = pbo_result["status"] == "evaluated"
    if economically_evaluable_trials == 0:
        judge_status = "not_evaluable"
    elif dsr_evaluable and pbo_evaluable:
        judge_status = "evaluated"
    elif dsr_evaluable or pbo_evaluable:
        judge_status = "partially_evaluable"
    else:
        judge_status = "partially_evaluable" if economically_evaluable_trials >= 1 else "not_evaluable"

    return {
        "schema_version": spec.schema_version,
        "protocol": {"protocol_id": spec.protocol_id},
        "scope": {"experiment_id": experiment_id, "hypothesis_id": hypothesis_id},
        "comparison_scope": comparison_scope,
        "registry_population": registry_population,
        "included_trial_ids": included_trial_ids,
        "excluded_trial_ids": excluded_trial_ids,
        "input_economic_result_ids": input_economic_result_ids,
        "input_artifacts": input_artifacts,
        "holdout": {"status": "reserved_not_evaluated"},
        "dsr_trial_accounting": trial_accounting,
        "dsr_results": dsr_results,
        "pbo_result": pbo_result,
        "judge_status": judge_status,
        "ids": {"judge_id": judge_id},
    }


# RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01 -- DSR trial-count protocol
# version. Bump this whenever the effective-independent-trial ESTIMATION
# METHOD changes (not when the registry population changes -- that's
# registry_population's job, and not when a numeric result changes -- see
# judge_id_basis's own docstring on why numeric results never enter identity
# fields). Distinct from JudgeSpec.schema_version, which governs the overall
# judge artifact shape.
DSR_TRIAL_COUNT_PROTOCOL_VERSION = "dsr_effective_trial_accounting_v1"
TRIAL_CORRELATION_METHOD = "bailey_lopez_de_prado_2014_appendix_a3_equal_weighted_average_pairwise_correlation"
CORRELATION_BASIS = "economic_walk_forward_v1_net_daily_return_excess_of_daily_risk_free_aligned_by_comparison_scope_dates"


def _compute_dsr_trial_accounting(
    candidates: List[_LoadedCandidate], per_trial_excess: Dict[str, np.ndarray]
) -> Dict[str, Any]:
    """Shared, group-level trial-count accounting consumed by every entry in
    dsr_results (RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01 / P6). Computed
    ONCE here (not re-derived per trial) so every dsr_results row is
    provably reading the same effective-N estimate -- never three silently
    different trial counts.

    Distinguishes, per the repair mission's required semantics:
      - raw_unique_trial_count: M, the RAW candidate/trial population size
        feeding this DSR computation (the economically-evaluable candidate
        count -- a DIFFERENT concept from registry_population's registry-
        truth fields, which this block does not replace or duplicate).
      - effective_independent_trial_count: N-hat, the count used INSIDE the
        DSR multiple-testing correction (Bailey & Lopez de Prado 2014,
        Appendix A.3) -- may be None when not numerically defensible.
    """
    trial_ids = sorted(c.trial_id for c in candidates)
    m = len(trial_ids)
    base = {
        "raw_unique_trial_count": m,
        "effective_independent_trial_count": None,
        "average_pairwise_correlation": None,
        "trial_correlation_method": TRIAL_CORRELATION_METHOD,
        "correlation_basis": CORRELATION_BASIS,
        "observations_used_for_correlation": None,
        "ill_conditioning_threshold": None,
        "numerically_defensible": False,
        "not_evaluable_reason": None,
        "dsr_trial_count_protocol_version": DSR_TRIAL_COUNT_PROTOCOL_VERSION,
    }
    if m < 2:
        return {**base, "not_evaluable_reason": "insufficient_trial_population_for_correction"}

    matrix = np.column_stack([per_trial_excess[t] for t in trial_ids])
    estimate = mts.estimate_effective_independent_trials(matrix)
    base["observations_used_for_correlation"] = estimate["observations"]
    if not estimate["evaluable"]:
        return {
            **base,
            "average_pairwise_correlation": estimate.get("average_pairwise_correlation"),
            "not_evaluable_reason": estimate["reason"],
        }
    return {
        **base,
        "effective_independent_trial_count": estimate["effective_independent_trials"],
        "average_pairwise_correlation": estimate["average_pairwise_correlation"],
        "ill_conditioning_threshold": estimate["ill_conditioning_threshold"],
        "numerically_defensible": True,
    }


def _compute_dsr_results(
    candidates: List[_LoadedCandidate],
    per_trial_excess: Dict[str, np.ndarray],
    per_trial_stats: Dict[str, Dict[str, Any]],
    annualization_days: int,
    trial_accounting: Dict[str, Any],
) -> List[Dict[str, Any]]:
    results: List[Dict[str, Any]] = []
    trial_ids = sorted(c.trial_id for c in candidates)
    n = len(trial_ids)
    if n < 2:
        for trial_id in trial_ids:
            stats = per_trial_stats[trial_id]
            results.append({
                "trial_id": trial_id,
                "evaluable": False,
                "reason": "insufficient_trial_population_for_correction",
                "observations": stats.get("observations"),
                "observed_sharpe_per_period": stats.get("sharpe_per_period"),
                "trials_used_by_correction": n,
                "effective_independent_trials_used_by_correction": None,
            })
        return results

    n_hat = trial_accounting["effective_independent_trial_count"]
    if n_hat is None:
        # The raw population is >= 2, but the effective-independent-trial
        # estimate itself is not numerically defensible (ill-conditioned
        # correlation estimate, or a degenerate N-hat <= 1 from
        # near-duplicate candidates). Per the repair mission: never fall
        # back to silently treating raw M as independent N -- report a
        # typed not_evaluable result instead.
        reason = f"effective_trial_correction_not_evaluable:{trial_accounting['not_evaluable_reason']}"
        for trial_id in trial_ids:
            stats = per_trial_stats[trial_id]
            results.append({
                "trial_id": trial_id,
                "evaluable": False,
                "reason": reason,
                "observations": stats["observations"],
                "observed_sharpe_per_period": stats["sharpe_per_period"],
                "observed_sharpe_annualized": stats["sharpe_per_period"] * float(np.sqrt(annualization_days)),
                "skewness": stats["skewness"],
                "kurtosis_raw": stats["kurtosis_raw"],
                "trials_used_by_correction": n,
                "effective_independent_trials_used_by_correction": None,
            })
        return results

    sr_list = [per_trial_stats[t]["sharpe_per_period"] for t in trial_ids]
    group_stats = mts.expected_max_sharpe(sr_list, effective_independent_trials=n_hat)
    e_max = group_stats["expected_max_sharpe"]
    variance = group_stats["variance_of_trial_sharpe"]

    for trial_id in trial_ids:
        stats = per_trial_stats[trial_id]
        psr = mts.probabilistic_sharpe_ratio(
            sharpe_hat=stats["sharpe_per_period"],
            sharpe_benchmark=e_max,
            observations=stats["observations"],
            skewness=stats["skewness"],
            kurtosis_raw=stats["kurtosis_raw"],
        )
        results.append({
            "trial_id": trial_id,
            "evaluable": psr is not None,
            "reason": None if psr is not None else "degenerate_psr_denominator",
            "observations": stats["observations"],
            "observed_sharpe_per_period": stats["sharpe_per_period"],
            "observed_sharpe_annualized": stats["sharpe_per_period"] * float(np.sqrt(annualization_days)),
            "skewness": stats["skewness"],
            "kurtosis_raw": stats["kurtosis_raw"],
            "trials_used_by_correction": n,
            "effective_independent_trials_used_by_correction": n_hat,
            "variance_of_trial_sharpe": variance,
            "expected_max_sharpe_benchmark": e_max,
            "deflated_sharpe_ratio": psr,
        })
    return results


def _compute_pbo_result(
    candidates: List[_LoadedCandidate],
    per_trial_excess: Dict[str, np.ndarray],
    spec: JudgeSpec,
) -> Dict[str, Any]:
    trial_ids = sorted(c.trial_id for c in candidates)
    n = len(trial_ids)
    if n < 2:
        return {"status": "not_evaluable", "reason": "insufficient_candidates_for_cscv", "num_candidates": n}

    t_obs = len(per_trial_excess[trial_ids[0]])
    block_count = mts.choose_cscv_block_count(t_obs, spec.cscv_target_block_count)
    if block_count is None:
        return {
            "status": "not_evaluable",
            "reason": "insufficient_observations_for_cscv_partition",
            "num_candidates": n,
            "num_observations": t_obs,
        }

    matrix = np.column_stack([per_trial_excess[t] for t in trial_ids])
    try:
        cscv = mts.combinatorial_symmetric_cv_pbo(matrix, block_count=block_count)
    except ValueError as exc:
        return {
            "status": "not_evaluable",
            "reason": f"cscv_computation_failed:{exc}",
            "num_candidates": n,
            "num_observations": t_obs,
        }

    return {
        "status": "evaluated",
        "num_candidates": n,
        "num_observations": t_obs,
        "trial_ids_column_order": trial_ids,
        **cscv,
    }


def run_multiple_testing_judge(
    out_path: Path,
    *,
    experiment_id: str,
    hypothesis_id: Optional[str] = None,
    registry_db: Path,
    spec: Optional[JudgeSpec] = None,
) -> Path:
    """Compute and persist the deterministic judge artifact at `out_path`."""
    out_path = Path(out_path)
    artifact = build_multiple_testing_judge(
        experiment_id=experiment_id, hypothesis_id=hypothesis_id, registry_db=registry_db, spec=spec
    )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(artifact, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path
