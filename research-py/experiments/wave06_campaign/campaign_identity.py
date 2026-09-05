"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 (Finding 1) -- single
shared source of truth for the Wave06 campaign's registry location and
REAL/PLACEBO experiment identity. Both candidate drivers
(wave06_candidate_liq01_amihud_illiquidity, wave06_candidate_vol01_volume_surprise)
import these constants directly -- neither redeclares its own copy -- so
their resolution is structurally, not just conventionally, identical.

Prior defect (Finding 1): each candidate previously owned its own
REAL_EXPERIMENT_ID and its own runs/run_01/registry/research.sqlite3 file.
build_multiple_testing_judge queries research_trials scoped by
experiment_id alone, so two candidates registered under two different
experiment_ids in two different SQLite files could never share a judge
population -- a later-executed candidate's own family judge structurally
could not see an earlier candidate's trials, producing campaign-level
winner-only accounting. Routing both candidates through ONE shared
registry file and ONE shared REAL/PLACEBO experiment_id makes the judge
population automatically the union of everything actually registered,
without either driver needing to know about the other's hypotheses.

See PREDECLARED_CAMPAIGN.json's "shared_campaign_registry" block for the
frozen source of these same values; test_campaign_predeclaration.py proves
byte-for-byte agreement between this module, that JSON, and each
candidate's own PREDECLARED_WAVE.json.
"""
from __future__ import annotations

from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
CAMPAIGN_RUN_ROOT = CAMPAIGN_ROOT / "runs" / "run_01"
CAMPAIGN_REGISTRY_DB = CAMPAIGN_RUN_ROOT / "registry" / "research.sqlite3"

CAMPAIGN_REAL_EXPERIMENT_ID = "WAVE06-CAMPAIGN-ALPHA-CANDIDATE-REAL-V1"
CAMPAIGN_PLACEBO_EXPERIMENT_ID = "WAVE06-CAMPAIGN-ALPHA-CANDIDATE-PLACEBOS-V1"


def resolve_local_src(experiment_file: Path) -> Path:
    """Fail-closed local-source resolution identical in spirit to each
    candidate driver's own resolve_wave03_checkout_local_src: import
    mqk_research from the SAME CHECKOUT that contains `experiment_file`,
    never from the checkout/worktree directory's basename. Raises
    RuntimeError -- never silently falls back -- if the sibling
    research-py/src structure is missing."""
    local_src = Path(experiment_file).resolve().parents[2] / "src"
    pkg_init = local_src / "mqk_research" / "__init__.py"
    if local_src.name != "src" or not pkg_init.is_file():
        raise RuntimeError(
            "refusing to run: expected a checkout-local research-py/src/mqk_research package "
            f"sibling to {experiment_file}, got {local_src}"
        )
    return local_src


def load_campaign(campaign_root: Path = CAMPAIGN_ROOT) -> dict:
    import json

    return json.loads((Path(campaign_root) / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))


def load_candidate_declaration(candidate_key: str, campaign_root: Path = CAMPAIGN_ROOT) -> dict:
    import json

    campaign = load_campaign(campaign_root)
    directory = campaign["candidates"][candidate_key]["directory"]
    path = (Path(campaign_root) / directory / "PREDECLARED_WAVE.json").resolve()
    return json.loads(path.read_text(encoding="utf-8"))
