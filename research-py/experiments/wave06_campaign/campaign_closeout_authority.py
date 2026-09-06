"""W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-03 (Finding 1) -- resolves a
Wave06 candidate's CANDIDATE_CLOSEOUT_STATUS.json evidence dict from REAL
ResearchResultStore/registry authority, never from a caller-supplied value.

Prior defect: campaign_order_guard.write_closeout_status() accepted an
arbitrary caller-built `evidence` dict, `hypothesis_ids`, and
`verified_trial_ids`. classify_verdict() applies the frozen policy to
whatever evidence it is handed, and load_verified_closeout() only checked
that the CITED trial_ids were genuinely registered/succeeded -- it never
checked that the numeric/boolean gate VALUES themselves came from anything
real. A fabricated all-pass evidence dict could therefore compute
DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION without any
real Research/P9 artifact ever having produced those values. Hashing that
dict (evidence_hash) proves only INTERNAL self-consistency, never authority.

This module is the ONLY sanctioned resolver of that evidence. It derives a
candidate's expected real hypothesis population itself (from
campaign_identity.load_candidate_declaration's own frozen
`real_candidate_population` -- never from a caller), and binds every gate to
one of:

  * a real, uniquely-registered ResearchResultStore trial and its terminal
    attempt (absolute_economic_requirement, matched_diagnostic_placebo_
    requirement, primary_vs_control_requirement -- each trial's own
    `attempt.artifact_paths_json` -> exact economic_walk_forward.json,
    cross-checked against that same attempt's own registry identity);
  * a real, already-registered `research_judge_artifacts` row (dsr_
    requirement, pbo_requirement) -- keyed by `judge_artifact_sha256`, the
    same durable authority `mqk-promotion`'s Rust verifier reads;
  * a real artifact file produced by the already-accepted evaluator CLI
    (genuine_shuffled_placebo_cli, p7a_p7b_economic_replay_stress_cli,
    dsr_pbo_sensitivity_cli) -- never an inline dict -- whose own
    self-declared trial_id/economic_eval_id/artifact-sha256 fields are
    cross-checked against the SAME resolved trial/attempt used above.

FINDING 1, EXPLICIT GAP (canonical_p9_robustness_gauntlet_requirement): no
Python-callable authority exists anywhere in this repo for the REAL
`bkt_robustness_gauntlet_v2` artifact. `RobustnessGauntletOutput::
is_complete` and `::all_applicable_passed`
(core-rs/crates/mqk-backtest/src/robustness_gauntlet.rs) are Rust-only
COMPUTED METHODS, not serialized struct fields -- there is no JSON key a
real artifact file could ever carry that Python could read to reproduce
either predicate, short of either (a) a new Rust-side JSON export of both
booleans, or (b) a from-scratch Python reimplementation of their pass/fail
logic (a "weaker parallel verifier", explicitly forbidden by this repair's
own mission). `resolve_authoritative_evidence` therefore raises
`MissingAuthoritativeSeam` -- never a caller-supplied placeholder -- the
moment it would need this gate's real value, i.e. only once every earlier
real gate has already been resolved and has genuinely passed. A genuine
REJECTED_NOT_ADVANCED at any EARLIER gate (insolvency, benchmark, matched
placebo, primary-vs-control, DSR, PBO, genuine shuffled placebo, or DSR/PBO
sensitivity) is fully resolvable today and never hits this wall, because
classify_verdict() itself never inspects a later gate once an earlier one
has terminally rejected.

The `benchmark_relative_requirement` gate has a similarly narrower gap: the
family-specific dynamic benchmark's own Sharpe (build_dynamic_rankable_
benchmark, in each candidate's own run_wave.py) is not a registered
ResearchResultStore trial at all -- there is no durable registry anchor for
it. The best available real authority is the candidate's own
`family_result.json`, written ONLY by the accepted run_family() driver's
--execute path (never hand-authored, per that driver's own module
contract) -- this module requires that file's path and cross-checks its
`long_short.registry` block against the SAME resolved trial/attempt.
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

from campaign_advancement_authority import _ALL_GATES, NOT_RUN, EvidenceRefusal, classify_verdict  # noqa: E402
from campaign_identity import load_campaign, load_candidate_declaration, resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

GROSS_WEALTH_INSOLVENCY_FAILURE_REASON = (
    "RuntimeError: Fail-closed: discrete gross wealth ledger equity is <= 0 -- "
    "cannot compute a further return fraction"
)


class AuthorityRefusal(EvidenceRefusal):
    """Real registry/artifact evidence could not be resolved into a
    trustworthy value -- missing/ambiguous trial, identity mismatch,
    contradictory terminal attempts, or an artifact that does not bind to
    the resolved trial/attempt it claims to. Never falls back to a
    caller-supplied value; the caller only ever supplies IDENTITY/LOCATION
    inputs (a sha256, a file path), never a gate result."""


class MissingAuthoritativeSeam(AuthorityRefusal):
    """The repo genuinely lacks a callable Python authority for this gate
    (see module docstring, canonical_p9_robustness_gauntlet_requirement).
    Raised instead of silently falling back to a caller assertion or a new,
    unaccepted parallel verifier."""


def _load_json(path: Path) -> Dict[str, Any]:
    path = Path(path)
    if not path.is_file():
        raise AuthorityRefusal(f"required artifact file does not exist: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise AuthorityRefusal(f"required artifact file is not readable/valid JSON: {path} ({exc})") from exc
    if not isinstance(data, dict):
        raise AuthorityRefusal(f"required artifact file did not contain a JSON object: {path}")
    return data


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def resolve_family_hypothesis_ids(candidate_key: str, campaign_root: Path = CAMPAIGN_ROOT) -> Tuple[str, str, str]:
    """Derives (hypothesis_id_long_only, hypothesis_id_long_short,
    hypothesis_id_placebo) from the candidate's OWN frozen
    PREDECLARED_WAVE.json -- never from a caller. Cross-checks the named
    long_only/long_short pair against the SAME declaration's own
    `real_candidate_population` so the two can never silently disagree."""
    decl = load_candidate_declaration(candidate_key, campaign_root)
    hyp_block = (decl.get("hypotheses") or {}).get(candidate_key)
    if not isinstance(hyp_block, dict):
        raise AuthorityRefusal(
            f"{candidate_key!r}'s own PREDECLARED_WAVE.json has no hypotheses[{candidate_key!r}] block"
        )
    hyp_lo = hyp_block.get("hypothesis_id_long_only")
    hyp_ls = hyp_block.get("hypothesis_id_long_short")
    hyp_pb = hyp_block.get("hypothesis_id_placebo")
    if not (hyp_lo and hyp_ls and hyp_pb):
        raise AuthorityRefusal(f"{candidate_key!r}'s hypotheses block is missing a required hypothesis_id_* field")
    real_population = decl.get("real_candidate_population")
    if not isinstance(real_population, list) or set(real_population) != {hyp_lo, hyp_ls}:
        raise AuthorityRefusal(
            f"{candidate_key!r}'s declared real_candidate_population {real_population!r} does not exactly "
            f"match its own hypothesis_id_long_only/hypothesis_id_long_short pair {{{hyp_lo!r}, {hyp_ls!r}}}"
        )
    return hyp_lo, hyp_ls, hyp_pb


def resolve_trial(store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str) -> Dict[str, Any]:
    """Exactly one registered trial for this (experiment_id, hypothesis_id)
    -- never zero, never more than one (Finding 3: a hypothesis id that
    belongs to a DIFFERENT candidate, or that was never attempted at all,
    structurally cannot resolve here)."""
    trials = store.list_trials(experiment_id=experiment_id, hypothesis_id=hypothesis_id)
    if len(trials) != 1:
        raise AuthorityRefusal(
            f"expected exactly one registered trial for experiment_id={experiment_id!r} "
            f"hypothesis_id={hypothesis_id!r}, found {len(trials)}"
        )
    return trials[0]


def resolve_optional_trial(store: ResearchResultStore, *, experiment_id: str, hypothesis_id: str) -> Optional[Dict[str, Any]]:
    """Like resolve_trial, but a genuinely NEVER-ATTEMPTED hypothesis (zero
    registered trials) is a legitimate "not evaluable" outcome rather than
    an authority refusal -- used only for the diagnostic placebo population,
    whose matched_diagnostic_placebo_requirement gate already has its own
    fail_closed_not_evaluable semantics in classify_verdict(). More than one
    registered trial for the same hypothesis_id is still refused (that is
    never a legitimate state, unlike zero)."""
    trials = store.list_trials(experiment_id=experiment_id, hypothesis_id=hypothesis_id)
    if len(trials) == 0:
        return None
    if len(trials) != 1:
        raise AuthorityRefusal(
            f"expected at most one registered trial for experiment_id={experiment_id!r} "
            f"hypothesis_id={hypothesis_id!r}, found {len(trials)}"
        )
    return trials[0]


def resolve_attempt_outcome(store: ResearchResultStore, trial_id: str) -> Dict[str, Any]:
    """Classifies a trial's own terminal attempt history against real
    registry rows only:

      "succeeded"           -- at least one succeeded attempt, and every
                                succeeded attempt shares the same result_id
                                (retries of an unchanged trial may not
                                silently disagree on outcome).
      "gross_insolvency_failed" -- no succeeded attempt, and at least one
                                failed attempt whose failure_reason is
                                EXACTLY the recognized gross-wealth-
                                insolvency string.
      "incomplete"           -- anything else (never attempted, only a
                                generic/operational failure, only a
                                'blocked'/'started' row) -- BLOCKS closeout,
                                never authorizes a terminal verdict.

    Fails closed (AuthorityRefusal) on CONTRADICTORY terminal evidence for
    the same trial_id -- e.g. one attempt succeeded while another failed
    with the recognized insolvency reason -- rather than silently preferring
    either outcome."""
    attempts = store.list_attempts(trial_id)
    succeeded = [a for a in attempts if a["status"] == "succeeded"]
    recognized_failed = [
        a for a in attempts if a["status"] == "failed" and a["failure_reason"] == GROSS_WEALTH_INSOLVENCY_FAILURE_REASON
    ]
    if succeeded and recognized_failed:
        raise AuthorityRefusal(
            f"trial_id={trial_id!r} has BOTH a succeeded attempt and a recognized gross-wealth-insolvency "
            "failed attempt -- contradictory terminal evidence for an unchanged trial, refusing to choose "
            "either outcome"
        )
    if succeeded:
        result_ids = {a["result_id"] for a in succeeded}
        if len(result_ids) != 1:
            raise AuthorityRefusal(
                f"trial_id={trial_id!r} has multiple succeeded attempts with disagreeing result_id values "
                f"{sorted(result_ids)!r} -- contradictory terminal evidence"
            )
        return {"status": "succeeded", "attempt": succeeded[-1]}
    if recognized_failed:
        return {"status": "gross_insolvency_failed", "attempt": recognized_failed[-1]}
    return {"status": "incomplete", "attempt": None}


def resolve_succeeded_economic_evidence(store: ResearchResultStore, trial: Dict[str, Any], attempt: Dict[str, Any]) -> Dict[str, Any]:
    """Reads the attempt's OWN durable `artifact_paths_json` ->
    economic_walk_forward.json (never a caller-supplied path), and
    cross-checks the file's own `registry` block and `ids.economic_eval_id`
    against the SAME resolved trial/attempt identity before trusting its
    `aggregate.net_sharpe`."""
    artifact_paths = json.loads(attempt.get("artifact_paths_json") or "{}")
    econ_path = artifact_paths.get("economic_walk_forward")
    if not econ_path:
        raise AuthorityRefusal(
            f"succeeded attempt_id={attempt['attempt_id']!r} has no artifact_paths.economic_walk_forward"
        )
    econ = _load_json(Path(econ_path))
    registry = econ.get("registry") or {}
    if (
        registry.get("trial_id") != trial["trial_id"]
        or registry.get("hypothesis_id") != trial["hypothesis_id"]
        or registry.get("experiment_id") != trial["experiment_id"]
        or registry.get("attempt_id") != attempt["attempt_id"]
        or registry.get("status") != "succeeded"
    ):
        raise AuthorityRefusal(
            f"economic_walk_forward.json registry identity at {econ_path} does not match the resolved "
            f"trial/attempt: expected trial_id={trial['trial_id']!r} attempt_id={attempt['attempt_id']!r}, "
            f"got {registry!r}"
        )
    economic_eval_id = (econ.get("ids") or {}).get("economic_eval_id")
    if not economic_eval_id or economic_eval_id != attempt.get("result_id"):
        raise AuthorityRefusal(
            f"economic_walk_forward.json ids.economic_eval_id ({economic_eval_id!r}) at {econ_path} does not "
            f"match the durable registry result_id ({attempt.get('result_id')!r}) for attempt_id="
            f"{attempt['attempt_id']!r}"
        )
    aggregate = econ.get("aggregate")
    if not isinstance(aggregate, dict) or aggregate.get("net_sharpe") is None:
        raise AuthorityRefusal(f"economic_walk_forward.json at {econ_path} has no aggregate.net_sharpe")
    return {
        "net_sharpe": float(aggregate["net_sharpe"]),
        "economic_eval_id": economic_eval_id,
        "economic_walk_forward_path": Path(econ_path),
    }


def resolve_judge_dsr_pbo(
    store: ResearchResultStore, *, judge_artifact_sha256: str, experiment_id: str, primary_trial_id: str
) -> Dict[str, Any]:
    """Binds DSR/PBO to a real, already-registered `research_judge_artifacts`
    row -- never a caller-copied DSR/PBO pair. `get_judge_artifact` raises
    KeyError (re-raised here as AuthorityRefusal) if the sha256 is unknown."""
    try:
        row = store.get_judge_artifact(judge_artifact_sha256)
    except KeyError as exc:
        raise AuthorityRefusal(f"unknown judge_artifact_sha256: {judge_artifact_sha256!r}") from exc
    if row["experiment_id"] != experiment_id:
        raise AuthorityRefusal(
            f"judge_artifact_sha256={judge_artifact_sha256!r} is registered under experiment_id="
            f"{row['experiment_id']!r}, expected {experiment_id!r}"
        )
    canonical_text = row["canonical_judge_json"]
    if hashlib.sha256(canonical_text.encode("utf-8")).hexdigest() != judge_artifact_sha256:
        raise AuthorityRefusal(
            f"registered judge artifact canonical_judge_json does not hash back to its own "
            f"judge_artifact_sha256={judge_artifact_sha256!r} -- registry integrity violation"
        )
    canonical = json.loads(canonical_text)
    included = canonical.get("included_trial_ids") or []
    if primary_trial_id not in included:
        raise AuthorityRefusal(
            f"primary trial_id={primary_trial_id!r} is not in this judge artifact's own included_trial_ids "
            f"{included!r} -- refusing to bind DSR/PBO evidence to a population that never covered it"
        )
    dsr_entry = next((r for r in (canonical.get("dsr_results") or []) if r.get("trial_id") == primary_trial_id), None)
    if dsr_entry is None:
        raise AuthorityRefusal(f"judge artifact has no dsr_results entry for primary trial_id={primary_trial_id!r}")
    pbo_result = canonical.get("pbo_result") or {}
    return {
        "dsr_evaluable": bool(dsr_entry.get("evaluable")),
        "dsr_value": dsr_entry.get("deflated_sharpe_ratio"),
        "pbo_evaluable": pbo_result.get("status") == "evaluated",
        "pbo_value": pbo_result.get("pbo"),
    }


def resolve_genuine_placebo_evidence(
    *, artifact_path: Path, primary_trial_id: str, expected_economic_eval_id: str, expected_economic_artifact_sha256: str
) -> Dict[str, Any]:
    """Binds genuine_shuffled_placebo_requirement to a real
    genuine_shuffled_placebo_cli output FILE -- never an inline dict --
    cross-checked against the SAME resolved trial/economic artifact."""
    data = _load_json(Path(artifact_path))
    if data.get("status") != "evaluated":
        return {"evaluable": False, "passed": None}
    if data.get("trial_id") != primary_trial_id:
        raise AuthorityRefusal(
            f"genuine shuffled placebo artifact trial_id={data.get('trial_id')!r} != resolved primary "
            f"trial_id={primary_trial_id!r}"
        )
    if data.get("baseline_economic_eval_id") != expected_economic_eval_id:
        raise AuthorityRefusal("genuine shuffled placebo artifact baseline_economic_eval_id does not match")
    if data.get("baseline_economic_artifact_sha256") != expected_economic_artifact_sha256:
        raise AuthorityRefusal("genuine shuffled placebo artifact baseline_economic_artifact_sha256 does not match")
    return {"evaluable": True, "passed": data.get("passed") is True}


def resolve_dsr_pbo_sensitivity_evidence(
    *, artifact_path: Path, primary_trial_id: str, judge_artifact_sha256: str, policy: Dict[str, Any]
) -> Dict[str, Any]:
    """Binds dsr_pbo_block_count_sensitivity_requirement to a real
    dsr_pbo_sensitivity_cli output FILE, reusing the SAME registered judge
    scope (authoritative_judge_artifact_sha256) as the baseline DSR/PBO
    gate, and the exact frozen block_counts grid -- never a caller-copied
    dsr_range/pbo_range pair."""
    data = _load_json(Path(artifact_path))
    if data.get("status") != "evaluated":
        return {"evaluable": False, "dsr_range": None, "pbo_range": None}
    if data.get("trial_id") != primary_trial_id:
        raise AuthorityRefusal(
            f"dsr_pbo_sensitivity artifact trial_id={data.get('trial_id')!r} != resolved primary "
            f"trial_id={primary_trial_id!r}"
        )
    if data.get("authoritative_judge_artifact_sha256") != judge_artifact_sha256:
        raise AuthorityRefusal(
            "dsr_pbo_sensitivity artifact authoritative_judge_artifact_sha256 does not match the same "
            "judge scope used for the baseline dsr_requirement/pbo_requirement gates"
        )
    if list(data.get("block_counts") or []) != list(policy["block_counts"]):
        raise AuthorityRefusal(
            f"dsr_pbo_sensitivity artifact block_counts {data.get('block_counts')!r} != frozen policy "
            f"block_counts {policy['block_counts']!r}"
        )
    return {"evaluable": True, "dsr_range": data.get("dsr_range"), "pbo_range": data.get("pbo_range")}


def resolve_authoritative_evidence(
    candidate_key: str,
    *,
    registry_db: Path,
    campaign_root: Path = CAMPAIGN_ROOT,
    benchmark_artifact_path: Optional[Path] = None,
    judge_artifact_sha256: Optional[str] = None,
    genuine_placebo_artifact_path: Optional[Path] = None,
    dsr_pbo_sensitivity_artifact_path: Optional[Path] = None,
) -> Tuple[Dict[str, Any], list, Dict[str, str]]:
    """Returns (evidence, hypothesis_ids, verified_trial_ids), every field
    derived from real registry/artifact authority -- see module docstring.
    Raises AuthorityRefusal (including its MissingAuthoritativeSeam
    subclass) rather than ever falling back to a caller-supplied value. The
    `benchmark_artifact_path`/`judge_artifact_sha256`/
    `genuine_placebo_artifact_path`/`dsr_pbo_sensitivity_artifact_path`
    location inputs are only REQUIRED once the cascade genuinely reaches a
    gate that needs them -- an insolvency-terminal candidate needs none of
    them.

    W06-FINAL-CLOSEOUT-LAZY-AUTHORITY-REPAIR-01: this is honored generally,
    not only for the insolvency gate. After each real gate is resolved, the
    resolver probes classify_verdict with every not-yet-resolved gate filled
    with a guaranteed-fail placeholder (the same pattern the canonical_p9
    gate below already used) and checks whether the NEXT gate in _ALL_GATES
    order comes back NOT_RUN_AFTER_DETERMINISTIC_REJECTION -- if so, an
    earlier REAL gate has already terminally decided the candidate, and this
    function returns immediately rather than demanding downstream authority
    the frozen classifier will never inspect. A downstream gate that IS
    genuinely reached (the placeholder itself gets evaluated, not skipped)
    still fails closed exactly as before -- this never makes real evidence
    optional for a gate the cascade actually reaches."""
    campaign = load_campaign(campaign_root)
    registry = campaign["shared_campaign_registry"]
    experiment_id = registry["real_experiment_id"]
    placebo_experiment_id = registry["placebo_experiment_id"]
    store = ResearchResultStore(Path(registry_db))
    policy = campaign["advancement_policy"]

    hyp_lo, hyp_ls, hyp_pb = resolve_family_hypothesis_ids(candidate_key, campaign_root)
    hypothesis_ids = sorted([hyp_lo, hyp_ls])

    trial_lo = resolve_trial(store, experiment_id=experiment_id, hypothesis_id=hyp_lo)
    trial_ls = resolve_trial(store, experiment_id=experiment_id, hypothesis_id=hyp_ls)
    verified_trial_ids = {hyp_lo: trial_lo["trial_id"], hyp_ls: trial_ls["trial_id"]}

    outcome_lo = resolve_attempt_outcome(store, trial_lo["trial_id"])
    outcome_ls = resolve_attempt_outcome(store, trial_ls["trial_id"])

    if outcome_lo["status"] == "incomplete" or outcome_ls["status"] == "incomplete":
        raise AuthorityRefusal(
            f"{candidate_key!r} long_only/long_short real trial(s) are not both complete "
            "(succeeded or recognized-gross-insolvency-failed) in the registry -- refusing to write a closeout"
        )

    def _not_evaluable_tail() -> Dict[str, Any]:
        return {
            "benchmark_relative_requirement": {"evaluable": False, "excess": None},
            "matched_diagnostic_placebo_requirement": {"evaluable": False, "excess": None},
            "primary_vs_control_requirement": {"evaluable": False, "excess": None},
            "dsr_requirement": {"evaluable": False, "value": None},
            "pbo_requirement": {"evaluable": False, "value": None},
            "genuine_shuffled_placebo_requirement": {"evaluable": False, "passed": None},
            "dsr_pbo_block_count_sensitivity_requirement": {"evaluable": False, "dsr_range": None, "pbo_range": None},
            "canonical_p9_robustness_gauntlet_requirement": {
                "protocol_version": None, "is_complete": False, "all_applicable_passed": False, "scenario_names": [],
            },
            "p7a_p7b_economic_replay_stress_requirement": {"evaluable": False, "passed": None},
        }

    def _tail_from(start_index: int) -> Dict[str, Any]:
        """Guaranteed-fail placeholders for every gate at/after
        _ALL_GATES[start_index] -- used both for a genuine early return and
        to probe whether classify_verdict actually reaches that gate."""
        remaining = set(_ALL_GATES[start_index:])
        return {k: v for k, v in _not_evaluable_tail().items() if k in remaining}

    def _cascade_already_terminated(evidence_prefix: Dict[str, Any], next_index: int) -> bool:
        """True if the frozen early-rejection cascade would already
        terminate before ever inspecting _ALL_GATES[next_index], using only
        the REAL evidence resolved so far. A guaranteed-fail placeholder
        fills every not-yet-resolved gate; if classify_verdict genuinely
        reaches _ALL_GATES[next_index] it evaluates (and fails) the
        placeholder rather than marking it NOT_RUN, which is exactly how
        this distinguishes "not reached" from "reached and would fail"."""
        probe_evidence = {**evidence_prefix, **_tail_from(next_index)}
        probe = classify_verdict(probe_evidence, policy)
        return probe["gates"][_ALL_GATES[next_index]] == NOT_RUN

    econ_gate = {
        "long_only_failure_reason": (
            GROSS_WEALTH_INSOLVENCY_FAILURE_REASON if outcome_lo["status"] == "gross_insolvency_failed" else None
        ),
        "long_short_failure_reason": (
            GROSS_WEALTH_INSOLVENCY_FAILURE_REASON if outcome_ls["status"] == "gross_insolvency_failed" else None
        ),
    }
    if econ_gate["long_only_failure_reason"] or econ_gate["long_short_failure_reason"]:
        evidence = {"absolute_economic_requirement": econ_gate, **_not_evaluable_tail()}
        return evidence, hypothesis_ids, verified_trial_ids

    econ_lo = resolve_succeeded_economic_evidence(store, trial_lo, outcome_lo["attempt"])
    econ_ls = resolve_succeeded_economic_evidence(store, trial_ls, outcome_ls["attempt"])

    # benchmark_relative_requirement is _ALL_GATES[1], immediately after
    # absolute_economic_requirement which has just PASSED -- it is
    # unconditionally reached, so its authority is unconditionally required.
    if benchmark_artifact_path is None:
        raise MissingAuthoritativeSeam(
            "benchmark_relative_requirement has no ResearchResultStore-anchored authority for the "
            "family-specific dynamic benchmark's own Sharpe -- pass benchmark_artifact_path (the "
            "candidate's own family_result.json, produced only by run_family()'s --execute path)"
        )
    family_result = _load_json(Path(benchmark_artifact_path))
    # W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-04 (emergent repair): the real
    # run_wave.py::run_family()/run_one_trial() output binds identity via
    # FLAT trial_id/hypothesis_id/experiment_id/economic_eval_id fields on
    # family_result["long_short"] itself -- there is no nested "registry"
    # sub-object anywhere in that file (confirmed against a real --execute
    # family_result.json; only economic_walk_forward.json, a SEPARATE file,
    # carries a "registry" block). economic_eval_id is itself a content-bound
    # identity already cross-verified against the resolved trial/attempt by
    # resolve_succeeded_economic_evidence (econ_ls, above) -- checking it here
    # is at least as strong a binding as the attempt_id/status pair this
    # replaces, and matches what real production output actually contains.
    fr_ls = family_result.get("long_short") or {}
    if (
        fr_ls.get("trial_id") != trial_ls["trial_id"]
        or fr_ls.get("hypothesis_id") != trial_ls["hypothesis_id"]
        or fr_ls.get("experiment_id") != trial_ls["experiment_id"]
        or fr_ls.get("economic_eval_id") != econ_ls["economic_eval_id"]
    ):
        raise AuthorityRefusal(
            f"family_result.json at {benchmark_artifact_path} long_short identity does not match "
            f"the resolved trial/attempt (trial_id={trial_ls['trial_id']!r} "
            f"hypothesis_id={trial_ls['hypothesis_id']!r} experiment_id={trial_ls['experiment_id']!r} "
            f"economic_eval_id={econ_ls['economic_eval_id']!r})"
        )
    benchmark_sharpe = (family_result.get("benchmark_long_short") or {}).get("sharpe")
    if benchmark_sharpe is None:
        raise AuthorityRefusal(f"family_result.json at {benchmark_artifact_path} has no benchmark_long_short.sharpe")
    benchmark_excess = econ_ls["net_sharpe"] - float(benchmark_sharpe)

    evidence_prefix: Dict[str, Any] = {
        "absolute_economic_requirement": econ_gate,
        "benchmark_relative_requirement": {"evaluable": True, "excess": benchmark_excess},
    }
    # matched_diagnostic_placebo_requirement is _ALL_GATES[2]: if benchmark
    # already terminally rejected (or was NOT_EVALUABLE), the cascade never
    # reaches it -- return now rather than resolving placebo/control at all.
    if _cascade_already_terminated(evidence_prefix, 2):
        evidence = {**evidence_prefix, **_tail_from(2)}
        return evidence, hypothesis_ids, verified_trial_ids

    trial_pb = resolve_optional_trial(store, experiment_id=placebo_experiment_id, hypothesis_id=hyp_pb)
    outcome_pb = resolve_attempt_outcome(store, trial_pb["trial_id"]) if trial_pb is not None else {"status": "incomplete"}
    if outcome_pb["status"] != "succeeded":
        placebo_gate_evidence = {"evaluable": False, "excess": None}
    else:
        econ_pb = resolve_succeeded_economic_evidence(store, trial_pb, outcome_pb["attempt"])
        placebo_gate_evidence = {"evaluable": True, "excess": econ_ls["net_sharpe"] - econ_pb["net_sharpe"]}

    control_excess = econ_ls["net_sharpe"] - econ_lo["net_sharpe"]

    evidence_prefix["matched_diagnostic_placebo_requirement"] = placebo_gate_evidence
    evidence_prefix["primary_vs_control_requirement"] = {"evaluable": True, "excess": control_excess}
    # dsr_requirement is _ALL_GATES[4]: if matched-placebo or control already
    # terminally rejected, the cascade never reaches dsr_requirement/
    # pbo_requirement -- judge authority is not required.
    if _cascade_already_terminated(evidence_prefix, 4):
        evidence = {**evidence_prefix, **_tail_from(4)}
        return evidence, hypothesis_ids, verified_trial_ids

    if judge_artifact_sha256 is None:
        raise AuthorityRefusal(
            "dsr_requirement/pbo_requirement require judge_artifact_sha256 (a real, already-registered "
            "research_judge_artifacts row) -- refusing to proceed without it"
        )
    dsr_pbo = resolve_judge_dsr_pbo(
        store, judge_artifact_sha256=judge_artifact_sha256, experiment_id=experiment_id,
        primary_trial_id=trial_ls["trial_id"],
    )
    evidence_prefix["dsr_requirement"] = {"evaluable": dsr_pbo["dsr_evaluable"], "value": dsr_pbo["dsr_value"]}
    evidence_prefix["pbo_requirement"] = {"evaluable": dsr_pbo["pbo_evaluable"], "value": dsr_pbo["pbo_value"]}
    # genuine_shuffled_placebo_requirement is _ALL_GATES[6]: if dsr or pbo
    # already terminally rejected, the cascade never reaches it.
    if _cascade_already_terminated(evidence_prefix, 6):
        evidence = {**evidence_prefix, **_tail_from(6)}
        return evidence, hypothesis_ids, verified_trial_ids

    if genuine_placebo_artifact_path is None:
        raise AuthorityRefusal(
            "genuine_shuffled_placebo_requirement requires genuine_placebo_artifact_path (a real "
            "genuine_shuffled_placebo_cli output file) -- refusing to proceed without it"
        )
    econ_ls_artifact_sha256 = _sha256_file(econ_ls["economic_walk_forward_path"])
    genuine_placebo_gate = resolve_genuine_placebo_evidence(
        artifact_path=genuine_placebo_artifact_path, primary_trial_id=trial_ls["trial_id"],
        expected_economic_eval_id=econ_ls["economic_eval_id"],
        expected_economic_artifact_sha256=econ_ls_artifact_sha256,
    )
    evidence_prefix["genuine_shuffled_placebo_requirement"] = genuine_placebo_gate
    # dsr_pbo_block_count_sensitivity_requirement is _ALL_GATES[7]: if the
    # genuine shuffled placebo already terminally rejected, the cascade
    # never reaches it.
    if _cascade_already_terminated(evidence_prefix, 7):
        evidence = {**evidence_prefix, **_tail_from(7)}
        return evidence, hypothesis_ids, verified_trial_ids

    if dsr_pbo_sensitivity_artifact_path is None:
        raise AuthorityRefusal(
            "dsr_pbo_block_count_sensitivity_requirement requires dsr_pbo_sensitivity_artifact_path (a "
            "real dsr_pbo_sensitivity_cli output file) -- refusing to proceed without it"
        )
    sensitivity_gate = resolve_dsr_pbo_sensitivity_evidence(
        artifact_path=dsr_pbo_sensitivity_artifact_path, primary_trial_id=trial_ls["trial_id"],
        judge_artifact_sha256=judge_artifact_sha256,
        policy=policy["dsr_pbo_block_count_sensitivity_requirement"],
    )
    evidence_prefix["dsr_pbo_block_count_sensitivity_requirement"] = sensitivity_gate

    # Every real, resolvable gate has now been computed from actual
    # registry/artifact authority. canonical_p9_robustness_gauntlet_
    # requirement is the one gate this repo cannot resolve at all (see
    # module docstring) -- but classify_verdict() itself only ever
    # INSPECTS that gate once every earlier one has already passed
    # (early_rejection_semantics). Rather than duplicate classify_verdict's
    # own threshold comparisons here (a second, potentially-diverging copy
    # of the frozen policy logic), PROBE it with a placeholder P9 block
    # that is guaranteed to fail if it is ever actually inspected
    # (is_complete=False): if classify_verdict already short-circuited at
    # an earlier REAL gate, gates["canonical_p9_robustness_gauntlet_
    # requirement"] comes back NOT_RUN regardless of the placeholder's
    # content, and this evidence is final and correct as-is. Only if the
    # probe shows P9 was actually REACHED (NOT_EVALUABLE_OR_FAILED) is the
    # true verdict genuinely unknown without real P9 evidence --
    # p7a_p7b_economic_replay_stress_requirement is evaluated strictly
    # after canonical_p9 in gate order, so it is likewise never reached
    # unless P9 itself would have passed, and needs no real artifact here.
    evidence = {**evidence_prefix, **_tail_from(8)}
    probe = classify_verdict(evidence, policy)
    if probe["gates"]["canonical_p9_robustness_gauntlet_requirement"] == NOT_RUN:
        return evidence, hypothesis_ids, verified_trial_ids

    raise MissingAuthoritativeSeam(
        f"{candidate_key!r} has cleared every real, resolvable gate (absolute economic solvency, "
        f"benchmark_excess={benchmark_excess!r}, placebo={placebo_gate_evidence!r}, "
        f"control_excess={control_excess!r}, dsr/pbo={dsr_pbo!r}, "
        f"genuine_placebo={genuine_placebo_gate!r}, sensitivity={sensitivity_gate!r}) -- but "
        "canonical_p9_robustness_gauntlet_requirement has NO callable Python authority in this repo: "
        "RobustnessGauntletOutput::is_complete/all_applicable_passed "
        "(core-rs/crates/mqk-backtest/src/robustness_gauntlet.rs) are Rust-only computed methods, never "
        "serialized JSON fields, and no Python loader/verifier for the bkt_robustness_gauntlet_v2 artifact "
        "exists anywhere in research-py. Accepting a caller-supplied is_complete/all_applicable_passed "
        "boolean here would be exactly the caller-assertion problem this repair exists to close. A "
        "follow-up patch must add a real seam (a Rust-side JSON export of both predicates, or a "
        "subprocess-invoked verifier) before a candidate that would otherwise ADVANCE or be INCONCLUSIVE "
        "can be authoritatively closed out."
    )
