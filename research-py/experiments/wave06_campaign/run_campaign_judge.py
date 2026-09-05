"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 (Finding 1) -- the ONLY
sanctioned way to run the Wave06 campaign's multiple-testing judge.

Prior defect: each candidate's own run_family_judge() computed its judge
population from ONLY its own two frozen hypothesis ids, under its own
private experiment_id/registry file. Once both candidates share one
registry and one REAL/PLACEBO experiment_id (campaign_identity.py), a
per-candidate population check would instead fail closed the moment a
second candidate's trials appear (unexpected extra hypotheses) -- which is
safe but not useful: no candidate driver could ever produce a correct
campaign-wide judge on its own. This module computes the correct
population directly: the union of every campaign_order candidate that has
ACTUALLY, LEGITIMATELY registered real trials so far, always inspecting
the FULL frozen campaign_order -- so a later candidate's judge run can
never silently omit an earlier one (or vice versa), and a "judge only the
candidate I care about" call is structurally impossible: the population is
never scoped to a single candidate's own hypotheses.

Both wave06_candidate_liq01_amihud_illiquidity/run_wave.py and
wave06_candidate_vol01_volume_surprise/run_wave.py's own `judge` stage
delegate to run_campaign_judge() here -- neither reimplements population
resolution.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

from campaign_identity import (  # noqa: E402
    CAMPAIGN_PLACEBO_EXPERIMENT_ID,
    CAMPAIGN_REAL_EXPERIMENT_ID,
    CAMPAIGN_REGISTRY_DB,
    CAMPAIGN_RUN_ROOT,
    load_campaign,
    load_candidate_declaration,
    resolve_local_src,
)

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge  # noqa: E402


def _family_hypothesis_ids(decl: dict) -> tuple[list[str], list[str]]:
    return decl["real_candidate_population"], decl["diagnostic_placebo_population"]


def _compute_attempted_population(
    store: ResearchResultStore, campaign: dict, *, experiment_id: str, population_index: int
) -> list[str]:
    """Union of every campaign_order candidate's hypothesis ids (real, when
    population_index==0; placebo, when population_index==1) that has been
    ACTUALLY, LEGITIMATELY attempted -- present means EVERY one of that
    candidate's own frozen hypothesis ids for this population has at least
    one registered trial; absent means none do. A candidate with SOME but
    not all of its own hypothesis ids present is a fail-closed defect
    (partial family registration) -- never silently included or excluded,
    and never allows a later candidate to be judged while omitting an
    earlier, already-attempted one (Finding 1, requirement D)."""
    registered = {t["hypothesis_id"] for t in store.list_trials(experiment_id=experiment_id)}
    expected: list[str] = []
    for key in campaign["campaign_order"]:
        decl = load_candidate_declaration(key, CAMPAIGN_ROOT)
        family_ids = _family_hypothesis_ids(decl)[population_index]
        present = [h for h in family_ids if h in registered]
        if present and len(present) != len(family_ids):
            raise RuntimeError(
                f"Fail-closed: campaign candidate {key!r} has partially registered trials for "
                f"experiment_id={experiment_id!r} ({present!r} present, expected all of {family_ids!r}) "
                "-- refusing to silently include or exclude a partial family from the judge population"
            )
        if present:
            expected.extend(family_ids)
    return expected


def compute_attempted_real_population(store: ResearchResultStore, campaign: dict) -> list[str]:
    return _compute_attempted_population(store, campaign, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, population_index=0)


def compute_attempted_placebo_population(store: ResearchResultStore, campaign: dict) -> list[str]:
    return _compute_attempted_population(
        store, campaign, experiment_id=CAMPAIGN_PLACEBO_EXPERIMENT_ID, population_index=1
    )


def _hypothesis_to_unique_trial_ids(store: ResearchResultStore, experiment_id: str) -> dict[str, set[str]]:
    by_hyp: dict[str, set[str]] = {}
    for t in store.list_trials(experiment_id=experiment_id):
        by_hyp.setdefault(t["hypothesis_id"], set()).add(t["trial_id"])
    return by_hyp


def require_exact_population(
    store: ResearchResultStore, *, experiment_id: str, expected_hypothesis_ids: list[str], population_label: str
) -> None:
    """Fail closed unless the durable registry holds EXACTLY the expected
    hypothesis population for `experiment_id` -- no missing candidate, no
    unexpected hypothesis (a placebo leaking into the real population, or
    an out-of-campaign hypothesis), and no duplicated semantic trial under
    a single hypothesis. Retries/attempts on the SAME trial_id never
    create a second entry."""
    by_hyp = _hypothesis_to_unique_trial_ids(store, experiment_id)
    expected = set(expected_hypothesis_ids)
    actual = set(by_hyp.keys())
    missing = expected - actual
    unexpected = actual - expected
    if missing or unexpected:
        raise RuntimeError(
            f"Fail-closed: {population_label} population for experiment_id={experiment_id!r} does not "
            f"exactly match the expected hypothesis set -- missing={sorted(missing)!r} "
            f"unexpected={sorted(unexpected)!r}"
        )
    duplicated = {h: sorted(ids) for h, ids in by_hyp.items() if len(ids) != 1}
    if duplicated:
        raise RuntimeError(
            f"Fail-closed: {population_label} population for experiment_id={experiment_id!r} has more "
            f"than one distinct trial registered under a single hypothesis id -- {duplicated!r}"
        )


def run_campaign_judge(*, registry_db: Path = CAMPAIGN_REGISTRY_DB) -> dict:
    """Computes the correct campaign-wide judge population from the shared
    registry and runs build_multiple_testing_judge over it. Fails closed if
    nothing has been attempted yet, or if any campaign candidate has only
    partially registered its own frozen hypothesis population."""
    campaign = load_campaign(CAMPAIGN_ROOT)
    store = ResearchResultStore(Path(registry_db))

    expected_real = compute_attempted_real_population(store, campaign)
    expected_placebo = compute_attempted_placebo_population(store, campaign)
    if not expected_real:
        raise RuntimeError(
            "Fail-closed: no campaign candidate has any attempted real trial yet -- nothing to judge"
        )
    require_exact_population(
        store, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, expected_hypothesis_ids=expected_real,
        population_label="real candidate",
    )
    require_exact_population(
        store, experiment_id=CAMPAIGN_PLACEBO_EXPERIMENT_ID, expected_hypothesis_ids=expected_placebo,
        population_label="diagnostic placebo",
    )

    judge = build_multiple_testing_judge(experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, registry_db=Path(registry_db))
    CAMPAIGN_RUN_ROOT.mkdir(parents=True, exist_ok=True)
    (CAMPAIGN_RUN_ROOT / "campaign_judge_artifact.json").write_text(
        json.dumps(judge, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    return judge


def main(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    if "--execute" not in argv:
        print(
            "REFUSED: the campaign judge requires the explicit --execute flag (hard execution guard).",
            file=sys.stderr,
        )
        raise SystemExit(3)
    run_campaign_judge()


if __name__ == "__main__":
    main()
