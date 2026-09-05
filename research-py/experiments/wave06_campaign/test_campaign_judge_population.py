"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 (Finding 1, requirements
C/D/E) -- proves the shared-registry judge population is the union of every
attempted campaign candidate, that a winner-only single-candidate population
is structurally refused once an earlier candidate was also attempted, and
that a placebo hypothesis never enters the real population. Disposable
synthetic registry fixtures only -- no real candidate directory or real
trial execution is touched.
"""
from __future__ import annotations

import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

import run_campaign_judge as campaign_judge  # noqa: E402
from campaign_identity import (  # noqa: E402
    CAMPAIGN_PLACEBO_EXPERIMENT_ID,
    CAMPAIGN_REAL_EXPERIMENT_ID,
    load_campaign,
    load_candidate_declaration,
    resolve_local_src,
)

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID  # noqa: E402


def _register_succeeded(store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str) -> str:
    trial_id = f"trial_{hypothesis_id}"
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    store.register_trial(
        trial_id=trial_id, experiment_id=experiment_id, hypothesis_id=hypothesis_id,
        strategy_id="fake_strategy_v1", protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": hypothesis_id},
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id)
    store.finalize_attempt(attempt_id, status="succeeded")
    return trial_id


def _liq_real_ids() -> list[str]:
    return load_candidate_declaration("LIQ-01")["real_candidate_population"]


def _vol_real_ids() -> list[str]:
    return load_candidate_declaration("VOL-01")["real_candidate_population"]


def test_population_is_liq_only_when_only_liq_attempted(tmp_path: Path) -> None:
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    for hyp in _liq_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)

    campaign = load_campaign()
    population = campaign_judge.compute_attempted_real_population(store, campaign)
    assert set(population) == set(_liq_real_ids())
    assert not set(population) & set(_vol_real_ids())


def test_population_includes_both_after_liq_fails_and_vol_is_also_attempted(tmp_path: Path) -> None:
    """Finding 1 requirement C: after LIQ-01 fails and VOL-01 is later
    attempted in the SAME campaign, the comparison population contains BOTH
    LIQ-01's and VOL-01's real trials."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    for hyp in _liq_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)
    for hyp in _vol_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)

    campaign = load_campaign()
    population = campaign_judge.compute_attempted_real_population(store, campaign)
    assert set(population) == set(_liq_real_ids()) | set(_vol_real_ids())
    assert len(population) == 4


def test_winner_only_vol_population_is_refused_when_liq_was_previously_attempted(tmp_path: Path) -> None:
    """Finding 1 requirement D: a VOL-only judge population is refused once
    LIQ was also attempted in this campaign -- require_exact_population
    fails closed on the "unexpected" LIQ hypotheses actually present in the
    shared registry, so a caller cannot construct a winner-only comparison
    merely by declaring a narrower expected set."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    for hyp in _liq_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)
    for hyp in _vol_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)

    try:
        campaign_judge.require_exact_population(
            store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID,
            expected_hypothesis_ids=_vol_real_ids(),  # winner-only, omitting LIQ
            population_label="real candidate",
        )
        assert False, "expected RuntimeError refusing the winner-only population"
    except RuntimeError as exc:
        assert "unexpected" in str(exc)

    # And compute_attempted_real_population itself never offers a VOL-only
    # view in the first place -- it always inspects the FULL campaign_order.
    campaign = load_campaign()
    population = campaign_judge.compute_attempted_real_population(store, campaign)
    assert set(_liq_real_ids()).issubset(set(population))


def test_placebo_hypothesis_never_enters_real_population(tmp_path: Path) -> None:
    """Finding 1 requirement E."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    for hyp in _liq_real_ids():
        _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp)
    placebo_ids = load_candidate_declaration("LIQ-01")["diagnostic_placebo_population"]
    for hyp in placebo_ids:
        _register_succeeded(store, experiment_id=CAMPAIGN_PLACEBO_EXPERIMENT_ID, hypothesis_id=hyp)

    campaign = load_campaign()
    real_population = campaign_judge.compute_attempted_real_population(store, campaign)
    assert not set(real_population) & set(placebo_ids)

    placebo_population = campaign_judge.compute_attempted_placebo_population(store, campaign)
    assert set(placebo_population) == set(placebo_ids)


def test_partial_family_registration_fails_closed(tmp_path: Path) -> None:
    """A candidate that registered only SOME of its own frozen real
    hypothesis ids is a fail-closed defect, never silently included or
    excluded."""
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    liq_ids = _liq_real_ids()
    _register_succeeded(store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=liq_ids[0])
    # liq_ids[1] deliberately never registered.

    campaign = load_campaign()
    try:
        campaign_judge.compute_attempted_real_population(store, campaign)
        assert False, "expected RuntimeError for partial family registration"
    except RuntimeError as exc:
        assert "partially registered" in str(exc)
