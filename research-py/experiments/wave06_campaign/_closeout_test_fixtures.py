"""Shared disposable fixture builders for test_campaign_order_guard.py and
test_campaign_closeout_authority.py. Leading-underscore module name so
pytest's default `test_*.py`/`*_test.py` collection never picks this file up
on its own -- it exists only to be imported.

Every fixture here builds REAL rows/files of the exact shape
campaign_closeout_authority.py actually reads (a real ResearchResultStore
trial/attempt with a real economic_walk_forward.json-shaped artifact file, a
real `research_judge_artifacts` row via `register_judge_artifact`, real
genuine_shuffled_placebo_cli/dsr_pbo_sensitivity_cli-shaped output files) --
never a caller-supplied evidence dict. No network, no real trial execution,
no real judge run.
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

from campaign_identity import CAMPAIGN_REAL_EXPERIMENT_ID, resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID  # noqa: E402

GROSS_WEALTH_INSOLVENCY_FAILURE_REASON = (
    "RuntimeError: Fail-closed: discrete gross wealth ledger equity is <= 0 -- "
    "cannot compute a further return fraction"
)

REAL_EXPERIMENT_ID = CAMPAIGN_REAL_EXPERIMENT_ID
PLACEBO_EXPERIMENT_ID = "FAKE-CAMPAIGN-PLACEBOS-01"

_REQUIRED_P9_SCENARIO_NAMES = [
    "execution_delay_stress",
    "symbol_leave_one_out",
    "month_year_regime_concentration",
    "parameter_neighborhood_execution",
    "placebo_temporal_offset",
    "conservative_capacity_stress",
    "dsr_pbo_sensitivity",
    "p7a_p7b_economic_replay_stress",
    "genuine_shuffled_placebo",
]

BLOCK_COUNTS = [8, 10, 12]


def minimal_policy() -> Dict[str, Any]:
    """A disposable synthetic advancement_policy, structurally identical in
    shape to PREDECLARED_CAMPAIGN.json's real one but with independent
    throwaway thresholds."""
    return {
        "benchmark_relative_requirement": {"min_excess": 0.0},
        "benchmark_relative_verdict_partition": {
            "rejected": "benchmark_excess <= 0.0",
            "inconclusive_band": "0.0 < benchmark_excess < 0.05",
            "gate_cleared": "benchmark_excess >= 0.05",
        },
        "matched_diagnostic_placebo_requirement": {"min_excess": 0.20},
        "primary_vs_control_requirement": {"min_excess": 0.0},
        "dsr_requirement": {"min_value": 0.5},
        "pbo_requirement": {"max_value": 0.5},
        "dsr_pbo_block_count_sensitivity_requirement": {
            "block_counts": BLOCK_COUNTS,
            "dsr_max_sensitivity_range": 0.15,
            "pbo_max_sensitivity_range": 0.15,
        },
        "canonical_p9_robustness_gauntlet_requirement": {
            "required_protocol_version": "bkt_robustness_gauntlet_v2",
            "required_scenario_names": _REQUIRED_P9_SCENARIO_NAMES,
        },
        "gross_wealth_insolvency_terminal_classification": {
            "recognized_failure_reason_strings": [GROSS_WEALTH_INSOLVENCY_FAILURE_REASON],
        },
    }


def _wave_declaration(candidate_key: str, *, hyp_lo: str, hyp_ls: str, hyp_pb: str) -> Dict[str, Any]:
    return {
        "real_candidate_population": [hyp_lo, hyp_ls],
        "diagnostic_placebo_population": [hyp_pb],
        "hypotheses": {
            candidate_key: {
                "hypothesis_id_long_only": hyp_lo,
                "hypothesis_id_long_short": hyp_ls,
                "hypothesis_id_placebo": hyp_pb,
            }
        },
    }


def fake_campaign_root(
    tmp_path: Path,
    *,
    candidate_keys: Optional[List[str]] = None,
) -> Path:
    """Builds a disposable campaign_root with PREDECLARED_CAMPAIGN.json
    (shared_campaign_registry + advancement_policy) and one
    PREDECLARED_WAVE.json per candidate, each with its own frozen
    hyp_<candidate>_long_only/long_short/placebo hypothesis ids."""
    candidate_keys = candidate_keys or ["A", "B"]
    root = tmp_path / "fake_campaign"
    root.mkdir()
    candidates: Dict[str, Any] = {}
    for key in candidate_keys:
        directory = f"cand_{key.lower()}"
        (root / directory).mkdir()
        hyp_lo = f"hyp_{key.lower()}_long_only"
        hyp_ls = f"hyp_{key.lower()}_long_short"
        hyp_pb = f"hyp_{key.lower()}_placebo"
        (root / directory / "PREDECLARED_WAVE.json").write_text(
            json.dumps(_wave_declaration(key, hyp_lo=hyp_lo, hyp_ls=hyp_ls, hyp_pb=hyp_pb)), encoding="utf-8"
        )
        candidates[key] = {"directory": directory}
    campaign = {
        "campaign_id": "FAKE-CAMPAIGN-01",
        "campaign_order": candidate_keys,
        "candidates": candidates,
        "shared_campaign_registry": {
            "real_experiment_id": REAL_EXPERIMENT_ID,
            "placebo_experiment_id": PLACEBO_EXPERIMENT_ID,
        },
        "advancement_policy": minimal_policy(),
    }
    (root / "PREDECLARED_CAMPAIGN.json").write_text(json.dumps(campaign), encoding="utf-8")
    return root


def candidate_hypothesis_ids(candidate_key: str) -> Dict[str, str]:
    key = candidate_key.lower()
    return {"long_only": f"hyp_{key}_long_only", "long_short": f"hyp_{key}_long_short", "placebo": f"hyp_{key}_placebo"}


def register_succeeded_economic_trial(
    store: ResearchResultStore,
    tmp_path: Path,
    *,
    experiment_id: str,
    hypothesis_id: str,
    trial_id: str,
    net_sharpe: float,
    strategy_id: str = "fake_strategy_v1",
) -> Dict[str, Any]:
    """Registers a real trial with a real succeeded attempt whose
    artifact_paths.economic_walk_forward points at a real, self-binding
    economic_walk_forward.json-shaped file."""
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": hypothesis_id},
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id)
    economic_eval_id = f"econ_{trial_id}"
    econ_path = tmp_path / f"{trial_id}_economic_walk_forward.json"
    econ_path.write_text(
        json.dumps({
            "ids": {"economic_eval_id": economic_eval_id},
            "registry": {
                "trial_id": trial_id, "hypothesis_id": hypothesis_id, "experiment_id": experiment_id,
                "attempt_id": attempt_id, "status": "succeeded",
            },
            "aggregate": {"net_sharpe": net_sharpe},
        }),
        encoding="utf-8",
    )
    store.finalize_attempt(
        attempt_id, status="succeeded", result_id=economic_eval_id,
        artifact_paths={"economic_walk_forward": str(econ_path)},
    )
    return {
        "trial_id": trial_id, "attempt_id": attempt_id, "economic_eval_id": economic_eval_id,
        "economic_walk_forward_path": econ_path,
        "economic_walk_forward_sha256": hashlib.sha256(econ_path.read_bytes()).hexdigest(),
    }


def register_insolvent_economic_trial(
    store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str, trial_id: str,
    strategy_id: str = "fake_strategy_v1", failure_reason: str = GROSS_WEALTH_INSOLVENCY_FAILURE_REASON,
) -> str:
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id=strategy_id, protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": hypothesis_id},
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id)
    store.finalize_attempt(attempt_id, status="failed", failure_reason=failure_reason)
    return attempt_id


def register_judge_artifact(
    store: ResearchResultStore, *, experiment_id: str, included_trial_ids: List[str],
    dsr_by_trial: Dict[str, Optional[float]], pbo_value: Optional[float], pbo_evaluable: bool = True,
    judge_id: str = "fake_judge_id",
) -> str:
    canonical = {
        "schema_version": "test-judge-v1",
        "protocol": {"protocol_id": "test-judge-protocol-v1"},
        "scope": {"experiment_id": experiment_id, "hypothesis_id": None},
        "included_trial_ids": included_trial_ids,
        "dsr_results": [
            {"trial_id": t, "evaluable": v is not None, "deflated_sharpe_ratio": v} for t, v in dsr_by_trial.items()
        ],
        "pbo_result": {"status": "evaluated" if pbo_evaluable else "not_evaluable", "pbo": pbo_value},
        "ids": {"judge_id": judge_id},
    }
    canonical_text = json.dumps(canonical, sort_keys=True, separators=(",", ":"))
    sha256 = hashlib.sha256(canonical_text.encode("utf-8")).hexdigest()
    store.register_judge_artifact(
        judge_id=judge_id, experiment_id=experiment_id, hypothesis_id=None, artifact_path=None,
        judge_artifact_sha256=sha256, canonical_judge_json=canonical_text,
        schema_version="test-judge-v1", protocol_id="test-judge-protocol-v1",
    )
    return sha256


def write_genuine_placebo_artifact(
    path: Path, *, trial_id: str, economic_eval_id: str, economic_artifact_sha256: str,
    passed: bool = True, status: str = "evaluated",
) -> Path:
    path.write_text(
        json.dumps({
            "status": status, "trial_id": trial_id, "baseline_economic_eval_id": economic_eval_id,
            "baseline_economic_artifact_sha256": economic_artifact_sha256, "passed": passed,
        }),
        encoding="utf-8",
    )
    return path


def write_sensitivity_artifact(
    path: Path, *, trial_id: str, judge_artifact_sha256: str, dsr_range: Optional[float],
    pbo_range: Optional[float], block_counts: Optional[List[int]] = None, status: str = "evaluated",
) -> Path:
    path.write_text(
        json.dumps({
            "status": status, "trial_id": trial_id, "authoritative_judge_artifact_sha256": judge_artifact_sha256,
            "block_counts": block_counts or BLOCK_COUNTS, "dsr_range": dsr_range, "pbo_range": pbo_range,
        }),
        encoding="utf-8",
    )
    return path


def write_family_result_artifact(
    path: Path,
    *,
    long_short_trial_id: str,
    long_short_hypothesis_id: str,
    long_short_experiment_id: str,
    long_short_economic_eval_id: str,
    benchmark_sharpe: float,
) -> Path:
    """Mirrors the REAL shape run_wave.py::run_family() actually writes:
    FLAT trial_id/hypothesis_id/experiment_id/economic_eval_id fields on
    family_result["long_short"] -- no nested "registry" sub-object (that only
    exists in the SEPARATE economic_walk_forward.json file)."""
    path.write_text(
        json.dumps({
            "long_short": {
                "trial_id": long_short_trial_id,
                "hypothesis_id": long_short_hypothesis_id,
                "experiment_id": long_short_experiment_id,
                "economic_eval_id": long_short_economic_eval_id,
            },
            "benchmark_long_short": {"sharpe": benchmark_sharpe},
        }),
        encoding="utf-8",
    )
    return path
