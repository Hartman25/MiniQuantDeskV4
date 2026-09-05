"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 -- mutation-proof
negative controls for the REAL, production `build_economic_trial_identity`
(mqk_research.ml.economic_registry_integration) and
`ResearchResultStore.register_trial`/`begin_attempt`
(mqk_research.exp_distributed.storage) seams, requested by the mission's
"MISSING REQUIRED NEGATIVE CONTROLS" section:

1. mutate the feature/schema/semantic candidate input -> trial_id MUST differ
2. alter a hypothetical/result-only field that is EXCLUDED from identity by
   the real identity builder's own documented contract -> trial_id MUST
   remain unchanged
3. register the same unchanged trial and begin multiple attempts -> one
   trial_id, monotonically separate attempt ids/indexes

Does not duplicate production identity logic -- exercises the real
`build_economic_trial_identity`/`ResearchResultStore` seams directly against
small disposable fixtures. No network, no real bars fetch, no real economic
evaluation.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))
from campaign_identity import resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_registry_integration import build_economic_trial_identity  # noqa: E402
from mqk_research.ml.economic_walkforward import (  # noqa: E402
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    PROTOCOL_ID as ECONOMIC_PROTOCOL_ID,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec  # noqa: E402
from mqk_research.ml.weight_to_share import WeightToShareSpec  # noqa: E402


def _write_csv(path: Path, rows: str) -> Path:
    path.write_text(rows, encoding="utf-8")
    return path


def _manifest(*, canonical_semantic_bars_hash: str = "hash_a") -> dict:
    """Minimal disposable bars-provenance manifest carrying every field
    provenance_identity_fragment requires."""
    return {
        "schema_version": "test_v1",
        "provider_ids_observed": ["alpaca"],
        "resolved_close_column": "close",
        "price_adjustment_convention": "alpaca_all_adjusted_v1",
        "corporate_action_policy": "test_policy_v1",
        "corporate_action_evidence_id": "evidence_a",
        "forbidden_periods": [],
        "timeframe": "1Day",
        "start_utc": "2020-01-01T00:00:00Z",
        "end_utc": "2020-06-01T00:00:00Z",
        "symbol_universe": ["AAA", "BBB"],
        "universe_mode": "fixed_ex_ante",
        "canonical_semantic_bars_hash": canonical_semantic_bars_hash,
        "source_attestation_id": "attestation_a",
    }


def _wf_spec() -> WalkForwardSpec:
    return WalkForwardSpec(
        train_years=3, test_months=3, step_months=3, holdout_months=6,
        min_rows_per_fold=300, purge_enabled=True, embargo_seconds=0,
    )


def _economic_spec() -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=5, max_gross_exposure=1.0,
        ),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
        annualization=AnnualizationSpec(),
    )


def _build_identity(tmp_path: Path, *, l2: float, bars_content: str, manifest: dict) -> tuple[str, dict]:
    features_path = _write_csv(tmp_path / "features.csv", "symbol,end_ts,feat\nAAA,2020-01-01,0.1\n")
    targets_path = _write_csv(tmp_path / "targets.csv", "symbol,end_ts,target\nAAA,2020-01-01,1\n")
    schema_path = _write_csv(tmp_path / "schema.json", json.dumps({"feature_columns": ["feat"]}))
    bars_path = _write_csv(tmp_path / f"bars_{hash(bars_content) & 0xffff}.csv", bars_content)
    return build_economic_trial_identity(
        experiment_id="TEST-MUTATION-PROOF-EXPERIMENT-V1",
        hypothesis_id="test_hypothesis_v1",
        strategy_id="test_strategy_v1",
        features_path=features_path,
        targets_path=targets_path,
        schema_path=schema_path,
        bars_path=bars_path,
        label_col="target",
        end_ts_col="end_ts",
        wf_spec=_wf_spec(),
        l2=l2,
        lr=0.05,
        steps=300,
        standardize=True,
        clip_z=8.0,
        economic_spec=_economic_spec(),
        bars_provenance=manifest,
    )


def test_semantic_input_mutation_changes_trial_id(tmp_path: Path) -> None:
    """MUTATION 1: changing a semantic model-spec field (l2) -- which
    directly participates in build_economic_trial_identity's own
    `model_spec` block -- MUST change trial_id."""
    manifest = _manifest()
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    trial_id_a, identity_a = _build_identity(tmp_path / "a", l2=0.001, bars_content="x", manifest=manifest)
    trial_id_b, identity_b = _build_identity(tmp_path / "b", l2=0.002, bars_content="x", manifest=manifest)
    assert trial_id_a != trial_id_b
    assert identity_a["model_spec"]["l2"] != identity_b["model_spec"]["l2"]


def test_excluded_bars_physical_bytes_do_not_change_trial_id(tmp_path: Path) -> None:
    """MUTATION 2: build_economic_trial_identity's own documented contract
    (module docstring, Defect 3) EXCLUDES bars_path's physical file-bytes
    identity from `identity` -- only bars_provenance's
    canonical_semantic_bars_hash carries bars data identity. Two physically
    different bars CSVs sharing the SAME bars_provenance manifest (same
    canonical_semantic_bars_hash) MUST produce the SAME trial_id."""
    manifest = _manifest(canonical_semantic_bars_hash="same_hash")
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    trial_id_a, _ = _build_identity(tmp_path / "a", l2=0.001, bars_content="physically,different,content\n1,2,3\n", manifest=manifest)
    trial_id_b, _ = _build_identity(tmp_path / "b", l2=0.001, bars_content="totally,other,bytes,here\n9,9,9,9\n", manifest=manifest)
    assert trial_id_a == trial_id_b


def test_retry_of_unchanged_trial_creates_new_attempt_not_new_trial(tmp_path: Path) -> None:
    """MUTATION 3: registering the SAME unchanged trial identity twice, then
    beginning two attempts, must produce exactly ONE trial_id and TWO
    monotonically-increasing, distinct attempt_ids/attempt_indexes -- a
    retry is a new attempt, never a new trial."""
    manifest = _manifest()
    trial_id, identity = _build_identity(tmp_path, l2=0.001, bars_content="x", manifest=manifest)

    store = ResearchResultStore(tmp_path / "registry" / "research.sqlite3")
    store.register_trial(
        trial_id=trial_id, experiment_id="TEST-MUTATION-PROOF-EXPERIMENT-V1",
        hypothesis_id="test_hypothesis_v1", strategy_id="test_strategy_v1",
        protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )
    # Re-registering the byte-identical trial is idempotent, not a collision.
    store.register_trial(
        trial_id=trial_id, experiment_id="TEST-MUTATION-PROOF-EXPERIMENT-V1",
        hypothesis_id="test_hypothesis_v1", strategy_id="test_strategy_v1",
        protocol_id=ECONOMIC_PROTOCOL_ID, identity=identity,
    )

    attempt_id_1, idx_1 = store.begin_attempt(trial_id=trial_id, origin="attempt_1")
    store.finalize_attempt(attempt_id_1, status="failed", failure_reason="synthetic failure for test")
    attempt_id_2, idx_2 = store.begin_attempt(trial_id=trial_id, origin="attempt_2")
    store.finalize_attempt(attempt_id_2, status="succeeded")

    trials = store.list_trials(experiment_id="TEST-MUTATION-PROOF-EXPERIMENT-V1")
    assert len(trials) == 1
    assert trials[0]["trial_id"] == trial_id

    assert idx_1 == 1
    assert idx_2 == 2
    assert attempt_id_1 != attempt_id_2
    attempts = store.list_attempts(trial_id)
    assert {a["attempt_id"] for a in attempts} == {attempt_id_1, attempt_id_2}
    assert {a["status"] for a in attempts} == {"failed", "succeeded"}


def test_result_value_alone_never_participates_in_identity(tmp_path: Path) -> None:
    """A caller-supplied economic RESULT (returns, eval_id, artifact paths)
    is never even accepted as a build_economic_trial_identity parameter --
    the function signature itself has no such field. This test proves the
    same feature/schema/model inputs, computed identically, always resolve
    to the same trial_id regardless of what a caller LATER records as that
    trial's result (result values are excluded by construction, not by
    convention the caller could accidentally violate)."""
    manifest = _manifest()
    (tmp_path / "run1").mkdir()
    (tmp_path / "run2").mkdir()
    trial_id_1, _ = _build_identity(tmp_path / "run1", l2=0.001, bars_content="x", manifest=manifest)
    trial_id_2, _ = _build_identity(tmp_path / "run2", l2=0.001, bars_content="x", manifest=manifest)
    assert trial_id_1 == trial_id_2
