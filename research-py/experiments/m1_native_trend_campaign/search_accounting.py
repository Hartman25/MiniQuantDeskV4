"""Cross-campaign search accounting for the M1 native research programme (read-only, pure).

Derives the cumulative disclosed search count from the committed, frozen declarations only; it
never reads a registry, a run directory, a result or a price. Definitions (a retry or a re-run
never manufactures a trial, a parameter change is never silently a new hypothesis):

* HYPOTHESIS  - an economic idea (one `strategy_id` family).
* TRIAL       - one registered candidate identity (hypothesis x instrument x frozen protocol).
* ATTEMPT     - one invocation of a trial; a retry is a new attempt of the SAME trial.
* EVALUATION  - a fold/slice/scenario evaluation of a registered trial; never a trial.

Three counts are reported and kept apart. The operative count is the registry-identity count
that excludes the superseded Batch 01 v1 evaluations; the other two are sensitivity bounds.

Pooled DSR/PBO across campaigns is NOT supported by the judge (its comparison scope binds the
bars provenance, including the symbol universe and the extraction range, and the historical
per-trial return series are not committed); `pooled_statistics_support` states that as BLOCKED.
"""

from __future__ import annotations

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
ACCOUNTING_SCHEMA = "m1_cross_campaign_search_accounting_v1"

OPERATIVE_RULE = "REGISTERED_NATIVE_TRIAL_IDENTITIES_EXCLUDING_SUPERSEDED_V1"
RULE_ALL_IDENTITIES = "ALL_REGISTERED_NATIVE_TRIAL_IDENTITIES_INCLUDING_SUPERSEDED_V1"
RULE_STRICT_DEDUP = "UNIQUE_STRATEGY_ID_SYMBOL_PAIRS"

# status classes (reviewer-authored, evidence-pinned by test_kiss_ext032_search_accounting.py)
STATUS_VOIDED = "VOIDED_INVALID_FOR_STATED_HYPOTHESIS"
STATUS_AMENDMENT = "AMENDMENT_OF_SAME_HYPOTHESIS_REJECTED"
STATUS_REJECTED = "REJECTED"
STATUS_SUPERSEDED_V1 = "SUPERSEDED_V1_REEVALUATED_BY_CORRECTED_DECLARATION"
STATUS_DISCOVERY_NOT_PROMOTED = "DISCOVERY_ADVANCED_NOT_ELIGIBLE"

# declaration file -> (status class, counts in the operative rule, evidence document)
PRIOR_DECLARATIONS: tuple[tuple[str, str, bool, str], ...] = (
    ("PREDECLARED_CAMPAIGN.json", STATUS_VOIDED, True, "docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md"),
    ("PREDECLARED_CAMPAIGN_02.json", STATUS_AMENDMENT, True, "docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md"),
    ("PREDECLARED_CAMPAIGN_03.json", STATUS_REJECTED, True, "docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md"),
    ("PREDECLARED_CAMPAIGN_DUAL_SMA_01.json", STATUS_REJECTED, True, "docs/research/M1_DUAL_SMA_CAMPAIGN_RESULT.md"),
    ("PREDECLARED_CAMPAIGN_PULLBACK_01.json", STATUS_REJECTED, True,
     "docs/research/M1_PULLBACK_MEAN_REVERSION_CAMPAIGN_RESULT.md"),
    ("PREDECLARED_BATCH_01.json", STATUS_SUPERSEDED_V1, False, "docs/research/M1_BATCH01_CORRECTED_RESULT.md"),
    ("PREDECLARED_BATCH_01_CORRECTED.json", STATUS_REJECTED, True, "docs/research/M1_BATCH01_CORRECTED_RESULT.md"),
    ("PREDECLARED_BATCH_02.json", STATUS_REJECTED, True, "docs/research/M1_BATCH02_RESULT.md"),
    ("PREDECLARED_BATCH_03.json", STATUS_DISCOVERY_NOT_PROMOTED, True,
     "docs/research/M1_BATCH03_DISCOVERY_RESULT.md"),
)

# broader same-window research exposure: disclosed, never mixed into the native denominator.
# (label, evidence document, a literal that must appear in it, population statement)
BROADER_EXPOSURE: tuple[tuple[str, str, str, str], ...] = (
    ("alpha_edge_census_01_corrected", "docs/research/ALPHA_EDGE_CENSUS_01_CORRECTED_RESULT.md", "38,192",
     "StrategyEdge 434 configurations x 88 symbols = 38,192 trials; Discovery [2016-01-01, 2024-01-01); labels/Python simulator"),
    ("alpha_census_02_strategy", "docs/research/ALPHA_CENSUS_02_RESULT.md", "9,400",
     "9,400 Strategy trials over 88 symbols, [2016, 2024); Python daily-bar simulator; DSR/PBO deferred"),
    ("alpha_census_02_conditional_factors", "docs/research/ALPHA_CENSUS_02_RESULT.md", "1,075",
     "1,075 conditional factors (forward-return labels, not executable P&L)"),
    ("alpha_edge_confirmation_01", "research-py/experiments/alpha_edge_confirmation_01/results/CONFIRMATION_RESULT.md",
     "87 eligible", "Conditional survivors on 87 of 88 symbols, window [2024-01-01, 2026-03-01) consumed"),
    ("alpha_edge_pass2_robustness_purge_01", "docs/research/ALPHA_EDGE_PASS2_ROBUSTNESS_PURGE_01_RESULT.md", "924",
     "924 robustness candidates (789 Strategy rows) derived from Census-01"),
    ("discovery_01_low_volatility", "docs/research/DISCOVERY_01_LOW_VOLATILITY_ANOMALY_RESULT.md", "88 symbols",
     "vol_rank_20 cross-sectional classifier on the 88-symbol seed universe"),
    ("short_01_etf_long_short_trend", "docs/research/SHORT_01_ETF_LONG_SHORT_TREND_REPORT.md", "SPY, QQQ, IWM,",
     "fixed 12-ETF universe including SPY, QQQ, IWM and DIA"),
    ("short_wave_02", "docs/research/SHORT_WAVE_02_FAMILY_REPORT.md", "SPY, QQQ, IWM, DIA",
     "three classifier hypotheses on the same 12-ETF universe, [2016-01-01, 2024-01-01)"),
)

SEED_UNIVERSE_FILES = (
    "research-py/experiments/short_wave_03_broad_direct_rank/SEED_UNIVERSE.json",
    "research-py/experiments/wave06_candidate_vol01_volume_surprise/SEED_UNIVERSE.json",
    "research-py/experiments/discovery_01_low_volatility_anomaly/SEED_UNIVERSE.json",
    "research-py/experiments/wave06_candidate_liq01_amihud_illiquidity/SEED_UNIVERSE.json",
)

CAMPAIGN_SYMBOLS = ("SPY", "QQQ", "IWM", "DIA")

# The population this accounting is FOR, stated here so `build_accounting` can reject a declaration that has
# drifted (a missing/extra slot, a variant, a changed trial budget) without trusting the declaration's own
# consistency. Changing it is a new campaign contract, not an edit.
NEW_CAMPAIGN_CONTRACT = {"hypotheses": 1, "strategy_id": "pre_holiday_two_session_long_v1",
                         "symbols": CAMPAIGN_SYMBOLS, "trials": 4, "max_trials": 4}
VALID_PRIOR_STATUSES = frozenset({STATUS_VOIDED, STATUS_AMENDMENT, STATUS_REJECTED, STATUS_SUPERSEDED_V1,
                                  STATUS_DISCOVERY_NOT_PROMOTED})
# Frozen declaration files in the experiment directory that are not search campaigns.
NON_CAMPAIGN_DECLARATION_FILES = frozenset({"PREDECLARED_BATCH_02_ERRATUM.json"})

FOUR_TRIAL_JUDGE_RESULT = "FOUR_TRIAL_JUDGE_RESULT"
CUMULATIVE_SEARCH_DISCLOSED = "CUMULATIVE_SEARCH_DISCLOSED"
CUMULATIVE_SEARCH_VALIDATED = "CUMULATIVE_SEARCH_VALIDATED"
CUMULATIVE_SEARCH_VALIDATION_BLOCKED = "CUMULATIVE_SEARCH_VALIDATION_BLOCKED"
POOLED_SUPPORTED = "SUPPORTED_AND_VERIFIED"
REPO_ROOT = HERE.parents[2]


def _strategy_ids(decl: dict) -> list[str]:
    if "hypotheses" in decl:
        return [h["strategy_id"] for h in decl["hypotheses"]]
    return [decl["native_engine"]["strategy_id"]]


def _trial_pairs(decl: dict) -> list[tuple[str, str]]:
    universe = decl["universe"]
    if "trials" in universe:
        return [(t["strategy_id"], t["symbol"]) for t in universe["trials"]]
    (strategy,) = _strategy_ids(decl)
    return [(strategy, s) for s in universe["symbols"]]


def _id(decl: dict) -> str:
    return decl.get("batch_id") or decl["campaign_id"]


def prior_inventory(root: Path = HERE) -> list[dict]:
    """One row per frozen prior declaration, in the fixed PRIOR_DECLARATIONS order."""
    rows = []
    for name, status, counts, evidence in PRIOR_DECLARATIONS:
        if status not in VALID_PRIOR_STATUSES:
            raise ValueError(f"{name}: invalid prior status {status!r}")
        if counts != (status != STATUS_SUPERSEDED_V1):
            raise ValueError(f"{name}: counts_in_operative_rule={counts} contradicts status {status}")
        if not (REPO_ROOT / evidence).is_file():
            raise ValueError(f"{name}: evidence document {evidence} does not exist")
        decl = json.loads((root / name).read_text(encoding="utf-8"))
        pairs = _trial_pairs(decl)
        declared = decl["universe"].get("max_trials")
        if declared is not None and declared != len(pairs):
            raise ValueError(f"{name}: max_trials {declared} != derived trial pairs {len(pairs)}")
        if len(set(pairs)) != len(pairs):
            raise ValueError(f"{name}: duplicate (strategy, symbol) trial inside one declaration")
        part = decl["partition"]
        rows.append({
            "declaration": name,
            "id": _id(decl),
            "experiment_id": decl["experiment"]["real_experiment_id"],
            "strategy_ids": sorted(set(_strategy_ids(decl))),
            "symbols": sorted(decl["universe"]["symbols"]),
            "registered_trial_identities": len(pairs),
            # derived from the frozen declaration; the historical registries are git-ignored run directories
            # and were not re-read, so "registered" here is the declaration's claim, not independent proof
            "identity_basis": "DECLARED_IN_FROZEN_DECLARATION",
            "registry_verified": False,
            "pairs": [list(p) for p in pairs],
            "status_class": status,
            "counts_in_operative_rule": counts,
            "evidence_document": evidence,
            "window": {"evaluation_start_utc": part["evaluation_start_utc"], "test_months": part["test_months"],
                       "holdout_months": part["holdout_months"], "expected_folds": part["expected_folds"]},
            "economic_protocol": decl["economic_protocol"]["signal_policy"]["direction_policy"],
            "capital_fraction_sizing": "capital_sizing" in decl,
        })
    return rows


def counts(inventory: list[dict], new_trials: int) -> dict:
    all_ids = sum(r["registered_trial_identities"] for r in inventory)
    operative = sum(r["registered_trial_identities"] for r in inventory if r["counts_in_operative_rule"])
    unique_pairs = len({tuple(p) for r in inventory for p in r["pairs"]})
    return {
        "prior_registered_trial_identities_all": all_ids,
        "prior_operative": operative,
        "prior_unique_strategy_symbol_pairs": unique_pairs,
        "new_trials": new_trials,
        "cumulative": {
            OPERATIVE_RULE: operative + new_trials,
            RULE_ALL_IDENTITIES: all_ids + new_trials,
            RULE_STRICT_DEDUP: unique_pairs + new_trials,
        },
        "operative_rule": OPERATIVE_RULE,
        "cumulative_disclosed_search_count": operative + new_trials,
    }


def pooled_statistics_support() -> dict:
    """Why a pooled cross-campaign DSR/PBO is not produced. Facts, not preferences."""
    return {
        "status": "BLOCKED_UNSUPPORTED",
        "pooled_dsr": None,
        "pooled_pbo": None,
        "reasons": [
            "judge comparison scope binds the bars provenance (symbol universe, extraction range, canonical bars hash): "
            "trials of campaigns with different universes are excluded as incompatible_comparison_scope",
            "the per-trial daily return series of the prior campaigns live in git-ignored run directories and registries, "
            "not in committed content, so no pooled return matrix can be reconstructed from the repository",
            "earlier campaigns used other economic protocols and sizing (long_only_v1 binary weights, fixed quantity), "
            "so their Sharpe estimates were produced by different measurement processes",
            "replacing the judge's trial count with the cumulative count would assume the unobserved prior Sharpe "
            "dispersion equals the new batch's: an unsupported arithmetic substitution",
        ],
        "supported_evaluation": "the batch-wide judge over the new experiment's four registered trials, which is "
                                "NOT deflated for the prior campaigns; the cumulative count is disclosed beside every DSR",
        "no_results_based_denominator_reduction": True,
    }


def _check_prior_set(root: Path, new_declaration: dict) -> None:
    """Every campaign declaration on disk other than the new one and the non-campaign files must be accounted
    for, and nothing accounted for may be absent: an omitted prior campaign shrinks the denominator."""
    own = {new_declaration.get("batch_id"), new_declaration.get("campaign_id")} - {None}
    on_disk = set()
    for path in sorted(Path(root).glob("PREDECLARED_*.json")):
        if path.name in NON_CAMPAIGN_DECLARATION_FILES:
            continue
        decl = json.loads(path.read_text(encoding="utf-8"))
        if own & {decl.get("batch_id"), decl.get("campaign_id")}:
            continue
        on_disk.add(path.name)
    accounted = {name for name, *_ in PRIOR_DECLARATIONS}
    if len(accounted) != len(PRIOR_DECLARATIONS):
        raise ValueError("a prior declaration is listed twice")
    omitted, phantom = sorted(on_disk - accounted), sorted(accounted - on_disk)
    if omitted or phantom:
        raise ValueError(f"prior campaigns omitted from the accounting {omitted}; accounted but absent {phantom}")


def validate_new_population(decl: dict) -> list[tuple[str, str]]:
    """The new declaration must be exactly the one-hypothesis, four-trial population this accounting covers.
    Returns its (strategy_id, symbol) slots in declared order, or raises ValueError."""
    c = NEW_CAMPAIGN_CONTRACT
    hyps = decl.get("hypotheses")
    if not isinstance(hyps, list) or len(hyps) != c["hypotheses"]:
        raise ValueError(f"hypothesis count {len(hyps) if isinstance(hyps, list) else hyps!r} != {c['hypotheses']}")
    if [h.get("strategy_id") for h in hyps] != [c["strategy_id"]]:
        raise ValueError("the declared hypothesis is not the contract's strategy (extra variant or substitution)")
    universe = decl["universe"]
    if tuple(universe.get("symbols", ())) != c["symbols"]:
        raise ValueError(f"unexpected symbols {universe.get('symbols')!r}; the contract is {list(c['symbols'])}")
    if universe.get("max_trials") != c["max_trials"]:
        raise ValueError(f"max_trials {universe.get('max_trials')!r} != {c['max_trials']}")
    trials = universe.get("trials")
    if not isinstance(trials, list):
        raise ValueError("universe.trials missing")
    pairs = [(t.get("strategy_id"), t.get("symbol")) for t in trials]
    if len(set(pairs)) != len(pairs):
        raise ValueError("duplicate (strategy, symbol) trial pair")
    expected = [(c["strategy_id"], s) for s in c["symbols"]]
    if len(pairs) != c["trials"] or pairs != expected:
        missing = [p for p in expected if p not in pairs]
        extra = [p for p in pairs if p not in expected]
        raise ValueError(f"trial slots differ from the contract (missing {missing}, unexpected {extra}, order matters)")
    if [t.get("order") for t in trials] != list(range(1, c["trials"] + 1)):
        raise ValueError("trial order fields must be exactly 1..N")
    return pairs


def validate_declared_block(decl: dict, accounting_counts: dict) -> None:
    """The declaration's own `search_accounting` block, when present, must equal the recomputed counts."""
    block = decl.get("search_accounting")
    if block is None:
        return
    pairs = (
        ("cumulative_disclosed_search_count", accounting_counts["cumulative_disclosed_search_count"]),
        ("prior_native_trial_identities_operative", accounting_counts["prior_operative"]),
        ("new_trials", accounting_counts["new_trials"]),
        ("sensitivity_counts", accounting_counts["cumulative"]),
        ("operative_rule", accounting_counts["operative_rule"]),
    )
    for key, value in pairs:
        if block.get(key) != value:
            raise ValueError(f"declared search_accounting.{key}={block.get(key)!r} contradicts the recomputed {value!r}")


def cumulative_search_validation(accounting: dict) -> str:
    """CUMULATIVE_SEARCH_VALIDATED only when a pooled method is supported AND verified; otherwise BLOCKED.
    Disclosure of the count is a separate, weaker fact (CUMULATIVE_SEARCH_DISCLOSED)."""
    if accounting.get("pooled_statistics_support", {}).get("status") == POOLED_SUPPORTED:
        return CUMULATIVE_SEARCH_VALIDATED
    return CUMULATIVE_SEARCH_VALIDATION_BLOCKED


def build_accounting(new_declaration: dict, root: Path = HERE) -> dict:
    pairs_new = validate_new_population(new_declaration)
    _check_prior_set(root, new_declaration)
    inv = prior_inventory(root)
    c = counts(inv, len(pairs_new))
    validate_declared_block(new_declaration, c)
    windows = {json.dumps(r["window"], sort_keys=True) for r in inv}
    if len(windows) != 1:
        raise ValueError("prior declarations do not share one evaluation window")
    prior_window = json.loads(next(iter(windows)))
    part = new_declaration["partition"]
    new_window = {k: part[k] for k in prior_window}
    if new_window != prior_window:
        raise ValueError(f"the new evaluation window {new_window} is not the prior exposed window {prior_window}")
    accounting = {
        "schema": ACCOUNTING_SCHEMA,
        "same_exposed_window": prior_window,
        "definitions": {
            "hypothesis": "an economic idea (one strategy_id family); this campaign adds exactly 1",
            "trial": "one registered candidate identity: hypothesis x instrument under the frozen protocol; this campaign adds exactly 4",
            "attempt": "one invocation of a trial; a retry is a new attempt of the SAME trial and never adds a trial",
            "evaluation": "a fold, slice or robustness scenario of a registered trial; never a trial",
            "retry": "infrastructure-failure re-invocation under a new attempt id; an economic failure is finalized failed and never retried",
            "failed_or_non_evaluable_trial": "stays in the population and counts worst; never dropped, never imputed",
            "cross_campaign_exposure": "prior trials on the same exposed window; disclosed in the cumulative count, never removed after results",
        },
        "counts": c,
        "identity_basis": {
            "prior_trial_identities": "DECLARED_IN_FROZEN_DECLARATIONS",
            "independently_verified_in_a_registry": None,
            "note": "the historical registries live in git-ignored run directories; the counts are the declarations' "
                    "claims cross-checked against their evidence documents, not a re-read of any registry",
        },
        "prior_inventory": [{k: v for k, v in r.items() if k != "pairs"} for r in inv],
        "broader_exposure": [
            {"label": label, "evidence_document": doc, "pinned_literal": literal, "population": text,
             "counted_in_native_denominator": False}
            for label, doc, literal, text in BROADER_EXPOSURE
        ],
        "campaign_symbols_in_88_symbol_seed_universe": list(CAMPAIGN_SYMBOLS),
        "pooled_statistics_support": pooled_statistics_support(),
    }
    validation = cumulative_search_validation(accounting)
    accounting["statistical_validation"] = {
        FOUR_TRIAL_JUDGE_RESULT: "the batch-wide judge over this experiment's four registered trials; produced only after "
                                 "an authorized execution and NOT deflated for the prior campaigns",
        CUMULATIVE_SEARCH_DISCLOSED: c["cumulative_disclosed_search_count"],
        "cumulative_search_status": validation,
        "statistical_acceptance_blocker": None if validation == CUMULATIVE_SEARCH_VALIDATED else {
            "id": "SAB-1", "requirement": "OD-4: the accepted multiple-testing requirement over the cumulative search",
            "status": "BLOCKED_UNSUPPORTED_BY_AVAILABLE_EVIDENCE",
            "effect": "no trial of this campaign may be labelled cumulative-search validated or qualifying; the "
                      "threshold (OD-4/OD-8) is not weakened"},
    }
    return accounting


if __name__ == "__main__":  # pragma: no cover - convenience printer
    decl = json.loads((HERE / "PREDECLARED_KISS_EXT032_ETF_01.json").read_text(encoding="utf-8"))
    print(json.dumps(build_accounting(decl)["counts"], indent=1))
