"""DISCOVERY-01-LOW-VOLATILITY-ANOMALY-PREDECLARATION-01 -- development-stage
Research experiment driver for ONE predeclared, genuinely new single-feature
OOS-classifier-score cross-sectional rank hypothesis (RISK-01: the
low-volatility / defensive anomaly -- Ang, Hodrick, Xing & Zhang (2006),
"The Cross-Section of Volatility and Expected Returns", Journal of Finance;
Frazzini & Pedersen (2014), "Betting Against Beta", Journal of Financial
Economics: leverage-constrained investors bid up high-beta/high-volatility
names for their embedded leverage, so low-volatility names have
historically earned superior risk-adjusted, and sometimes raw, returns),
run as a matched LONG-ONLY control / LONG-SHORT candidate pair plus a
causal same-horizon placebo, using the existing accepted Research ->
registered walk-forward -> economic evaluation -> multiple-testing-judge
pipeline, over the SAME 88-symbol broad Research universe seed used by
SHORT-WAVE-03 (see SEED_UNIVERSE.json; identical universe_id, reused
unchanged, not regenerated).

This driver is forked, with minimal targeted identity/config edits only,
from the already-reviewed, already-executed
research-py/experiments/short_wave_03_broad_direct_rank/run_wave.py (see
docs/research and the SHORT-WAVE-03 run_01 local evidence). All shared
pipeline logic (bars cache-authority verification, causal placebo
construction, the dynamic rankable benchmark state machine, recording-field
derivation) is REUSED VERBATIM AND UNCHANGED from that proven driver --
internal helper names below therefore still carry the literal token
"wave03" (e.g. resolve_wave03_checkout_local_src,
verify_wave03_bars_cache_authority); this is intentional, reduces diff risk
against a battle-tested (R1-R4 repaired) implementation, and has no bearing
on this experiment's own identity, which lives entirely in the frozen
constants below and in PREDECLARED_WAVE.json. Historical "R1"/"R2"/... patch
citations in docstrings/error text below refer to the ORIGINAL SHORT-WAVE-03
defect-repair patch IDs and are preserved verbatim as accurate provenance --
they describe where this exact logic came from, not this experiment.

The feature itself (`vol_rank_20`, the cross-sectional percentile rank of
20-day rolling realized volatility of daily log returns) is NOT new code --
it is already unconditionally computed by the existing, unmodified
mqk_research.features.feature_set_v1.build_feature_set_v1 (default
vol_windows=(10,20,60), default cross_section_windows=(5,20)) but has never
been used as the ranking feature in ALPHA-01 (momentum_score), SHORT-01
(slope_60), or SHORT-WAVE-02/03 (ret_rank_20, ret_5, gap_pct_1) -- it is a
genuinely untested, pre-existing, already-engineered feature representing a
risk-based mechanism with no definitional overlap with any previously
tested return/price-direction feature.

All experiment identity, seed universe, data window, label definition,
walk-forward spec, model hyperparameters, cost model, execution model,
rank_side_count, and the placebo seed are FROZEN in PREDECLARED_WAVE.json
BEFORE any trial result is observed. This file must not be edited to react
to an observed result.

CLASSIFICATION (mission): this wave is a NEW_MECHANISM_DEVELOPMENT_STUDY
over a non-point-in-time, fixed_ex_ante, current-registry-snapshot universe
(see docs/research/BROAD_RESEARCH_UNIVERSE_CURRENT_TRUTH_AUDIT.md) --
genuinely untested feature, but still a post-hoc-universe development study,
not a point-in-time-clean alpha proof. Maximum possible positive verdict:
DEVELOPMENT_PROMISING_REQUIRES_FRESH_CONFIRMATION. Never PROVEN_ALPHA or
PROMOTION_READY.

HARD EXECUTION GUARD: no stage in EXECUTE_REQUIRED_STAGES may run unless
the literal string "--execute" is present in argv (see main()). The `check`
stage never touches the network and never reads Alpaca credentials.

Uses ONLY existing, frozen, already-accepted production entry points:
  - mqk_research.data.alpaca_historical.extract_research_bars_with_provenance
  - mqk_research.features.feature_set_v1.build_feature_set_v1
  - mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
  - mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge

No research-py/src file is modified by this script.

The causal placebo helper (`build_causal_placebo_targets`) is an exact,
self-contained reproduction of the accepted, PUSHED-VERIFIED
research-alpha-gap-discovery-01-clean worktree's implementation, inlined
the same way SHORT-WAVE-02/03's own run_wave.py inlined it (portability
convention) so this driver is runnable from a bare checkout of this branch
alone. Semantics unchanged.

BENCHMARK: family-specific DYNAMIC equal-weight comparator, identical
formula/implementation to SHORT-WAVE-03's own `build_dynamic_rankable_
benchmark` (already implemented and exercised there -- see run_01 local
evidence's benchmark_type="dynamic_equal_weight_causally_rankable_fold_reset_v1"),
reused verbatim unchanged.

DATA FRESHNESS: unlike ALPHA-01/SHORT-01/SHORT-WAVE-02/03 (all windowed to
2016-01-01..2024-01-01), this experiment's FINAL frozen END_UTC is
2025-05-01 (ASOF=2025-05-01), still well past every prior experiment's
2024-01-01 cutoff. Development evidence (used to fit/select folds) ends at
the reserved holdout's own start; the reserved final holdout window is
2024-11-01..2025-05-01 (WF_HOLDOUT_MONTHS=6) and remains genuinely
untouched, never-before-seen.

The originally predeclared END_UTC=2026-09-05 endpoint was superseded,
BEFORE any trial result was observed, by a pre-outcome data-authority
amendment (commit 3cf711cf) that narrowed END_UTC to strictly before the
earliest unresolved 2025-2026 corporate-action "name_change" event (RKLB,
2025-05-27) -- see the END_UTC assignment below for the full provenance
account. No claim is made, and none should be inferred, that any 2026 bar,
feature, prediction, or economic result was ever computed or evaluated by
this campaign.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any, Optional


def resolve_wave03_checkout_local_src(experiment_file: Path) -> Path:
    """R3 (WAVE03-CHECKOUT-LOCAL-SOURCE-GUARD-01): fail-closed local-source
    safety must not depend on the checkout/worktree DIRECTORY NAME -- a
    temporary worktree's basename (e.g. "research-rank-wave-01") is not
    research authority, and hardcoding it made this guard pass only from
    that one specific worktree, failing closed (wrongly) once the same
    committed code ran from an ordinary checkout named anything else.

    This driver must import mqk_research from the SAME CHECKOUT that
    contains `experiment_file` (this module), whatever that checkout
    happens to be named: resolve the sibling checkout-local
    `research-py/src` from the file path, verify the required local
    package structure actually exists there, and return that exact
    resolved path (never the file's parent basename) for the caller to
    place first on sys.path. Raises RuntimeError -- never silently falls
    back to any other src/ -- if the sibling structure is missing/wrong."""
    local_src = Path(experiment_file).resolve().parents[2] / "src"
    pkg_init = local_src / "mqk_research" / "__init__.py"
    if local_src.name != "src" or not pkg_init.is_file():
        raise RuntimeError(
            "refusing to run: expected a checkout-local research-py/src/mqk_research package "
            f"sibling to {experiment_file}, got {local_src}"
        )
    return local_src


WAVE03_LOCAL_SRC = resolve_wave03_checkout_local_src(Path(__file__))
sys.path.insert(0, str(WAVE03_LOCAL_SRC))

import mqk_research as _wave03_local_mqk_research_check

# Defense-in-depth: verify the mqk_research actually imported (module
# resolution order can be shadowed by an installed package elsewhere on
# sys.path) resolves underneath THIS SAME checkout-local src/, not merely
# that inserting it succeeded.
_wave03_resolved_mqk_research = Path(_wave03_local_mqk_research_check.__file__).resolve()
if not _wave03_resolved_mqk_research.is_relative_to(WAVE03_LOCAL_SRC):
    raise RuntimeError(
        "refusing to run: imported mqk_research did not resolve underneath the checkout-local "
        f"src/ ({WAVE03_LOCAL_SRC}); got {_wave03_resolved_mqk_research}"
    )

import numpy as np
import pandas as pd

from mqk_research.data.bars_provenance import (
    check_corporate_action_integrity,
    require_bars_match_manifest,
    require_registered_bars_provenance,
)
from mqk_research.features.feature_set_v1 import build_feature_set_v1
from mqk_research.ml.economic_walkforward import (
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    load_oos_predictions,
)
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economics import compute_max_drawdown, compute_sharpe
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.weight_to_share import WeightToShareSpec

# ---------------------------------------------------------------------------
# Self-contained, exact reproduction of the accepted causal placebo helper.
# See module docstring for attribution/provenance. Semantics unchanged.
# ---------------------------------------------------------------------------


def build_causal_placebo_targets(targets: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    """Deterministic negative control that destroys symbol-specific
    predictive association WITHOUT moving information across temporal label
    horizons: permutes the (fwd_ret, target) PAIR only within rows sharing
    the EXACT same (end_ts, label_end_ts). Fails closed if zero rows'
    target value actually changed."""
    rng = np.random.default_rng(seed)
    out = targets.copy().reset_index(drop=True)
    original_target = out["target"].to_numpy(copy=True)
    fwd_ret = out["fwd_ret"].to_numpy(copy=True)
    target = out["target"].to_numpy(copy=True)

    group_indices = out.groupby(["end_ts", "label_end_ts"], sort=False).indices
    for key in sorted(group_indices.keys()):
        idx = group_indices[key]
        if len(idx) <= 1:
            continue
        perm = rng.permutation(len(idx))
        fwd_ret[idx] = fwd_ret[idx][perm]
        target[idx] = target[idx][perm]

    if int(np.sum(target != original_target)) == 0:
        raise RuntimeError(
            "Fail-closed: causal placebo produced zero changed target assignments -- not a "
            "valid classifier negative control."
        )
    out["fwd_ret"] = fwd_ret
    out["target"] = target
    return out


EXPERIMENT_ROOT = Path(__file__).resolve().parent
RUN_ROOT = EXPERIMENT_ROOT / "runs" / "run_01"
REGISTRY_DB = RUN_ROOT / "registry" / "research.sqlite3"
PREDECLARATION_PATH = EXPERIMENT_ROOT / "PREDECLARED_WAVE.json"
SEED_UNIVERSE_PATH = EXPERIMENT_ROOT / "SEED_UNIVERSE.json"

PRIMARY_PAPER_REPO = Path(r"C:\Users\Zacha\Desktop\MiniQuantDeskV4")
# Informational provenance only (not enforced anywhere in this file, same as
# in the SHORT-WAVE-03 driver this was forked from): the authoritative repo
# HEAD this predeclaration/execution was performed against.
PRIMARY_PAPER_HEAD_EXPECTED = "45fa89ecdda2e8f3d7c0d35141ae57e53c3255d7"

REAL_EXPERIMENT_ID = "DISCOVERY-01-LOW-VOLATILITY-ANOMALY-REAL-V1"
PLACEBO_EXPERIMENT_ID = "DISCOVERY-01-LOW-VOLATILITY-ANOMALY-PLACEBOS-V1"

START_UTC = pd.Timestamp("2016-01-01T00:00:00Z")
# NOTE: originally predeclared as 2026-09-05 (today, per Phase C's live SPY
# availability check). The first execution attempt at that end_utc hit the
# shared bars-provenance pipeline's fail-closed CorporateActionReviewRequired
# gate (mqk_research.data.alpaca_historical): three symbols (CCL, RKLB, XOM)
# carry unresolved 2025-2026 "name_change" corporate-action events (CUSIP
# relabelings) not covered by adjustment="all" semantics. This is a DATA
# PROVENANCE boundary discovered before any bars were fetched/persisted and
# before any strategy result was computed -- not a reaction to an economic
# outcome -- so END_UTC is narrowed here to strictly before the earliest such
# event (RKLB, 2025-05-27) rather than attempting to patch the shared
# corporate-action resolver (out of scope for this predeclaration). Still
# ~16 months fresher than every prior experiment's 2024-01-01 cutoff.
END_UTC = pd.Timestamp("2025-05-01T00:00:00Z")
ASOF = "2025-05-01"
TIMEFRAME = "1Day"
FEED = "sip"

LABEL_HORIZON_BARS = 10
LABEL_RET_THRESHOLD = 0.0

RANK_SIDE_COUNT = 5
MAX_GROSS_EXPOSURE = 1.0
PLACEBO_SEED = 4242

# R4 (WAVE03-RUN-RECORDING-TRUTH-REPAIR-01): the only holdout status any
# Wave-03 trial's own registered economic artifact may ever report -- this
# development-stage controller never consumes the final holdout (see module
# docstring). run_one_trial fails closed if the real evaluator output
# disagrees; compute_family_recording_fields derives the recorded
# "holdout_status" field FROM that verified value, never a bare literal.
RESERVED_NOT_EVALUATED_HOLDOUT_STATUS = "reserved_not_evaluated"

COMMISSION_BPS_PER_SIDE = 10.0
SLIPPAGE_BPS_PER_SIDE = 0.0
EXECUTION_SLIPPAGE_BPS = 5
EXECUTION_VOLATILITY_MULT_BPS = 0
EQUITY_USD = 100_000.0

WF_TRAIN_YEARS = 3
WF_TEST_MONTHS = 3
WF_STEP_MONTHS = 3
WF_HOLDOUT_MONTHS = 6
WF_MIN_ROWS_PER_FOLD = 300

MODEL_L2 = 0.001
MODEL_LR = 0.05
MODEL_STEPS = 300
MODEL_STANDARDIZE = True
MODEL_CLIP_Z = 8.0


def load_predeclaration() -> dict:
    return json.loads(PREDECLARATION_PATH.read_text(encoding="utf-8"))


def load_seed_universe() -> dict:
    return json.loads(SEED_UNIVERSE_PATH.read_text(encoding="utf-8"))


def seed_symbols() -> list[str]:
    """Frozen seed symbol list. Read from the committed SEED_UNIVERSE.json
    artifact ONLY -- never re-derived from the live registry at run time,
    so the seed stays exactly what was frozen before any result was seen,
    even if the live registry later changes."""
    return list(load_seed_universe()["symbols"])


class HypothesisFamily:
    def __init__(self, *, key: str, feature_column: str, strategy_id: str,
                 hyp_long_only: str, hyp_long_short: str, hyp_placebo: str) -> None:
        self.key = key
        self.feature_column = feature_column
        self.strategy_id = strategy_id
        self.hyp_long_only = hyp_long_only
        self.hyp_long_short = hyp_long_short
        self.hyp_placebo = hyp_placebo


FAMILIES: dict[str, HypothesisFamily] = {
    "RISK-01": HypothesisFamily(
        key="RISK-01", feature_column="vol_rank_20",
        strategy_id="pooled_single_feature_xs_low_volatility_direct_rank_v1",
        hyp_long_only="discovery01_risk01_low_volatility_anomaly_long_only_v1",
        hyp_long_short="discovery01_risk01_low_volatility_anomaly_long_short_v1",
        hyp_placebo="discovery01_risk01_low_volatility_anomaly_placebo_v1",
    ),
}

REAL_CANDIDATE_HYPOTHESIS_IDS = sorted(
    [f.hyp_long_only for f in FAMILIES.values()] + [f.hyp_long_short for f in FAMILIES.values()]
)
DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS = sorted(f.hyp_placebo for f in FAMILIES.values())


def assert_driver_agrees_with_predeclaration() -> None:
    """Fail closed unless every frozen constant in this module matches the
    committed PREDECLARED_WAVE.json / SEED_UNIVERSE.json byte-for-byte on
    every field that matters for research identity. See
    test_predeclaration.py for the executable proof."""
    decl = load_predeclaration()
    seed = load_seed_universe()

    assert decl["real_experiment_id"] == REAL_EXPERIMENT_ID
    assert decl["placebo_experiment_id"] == PLACEBO_EXPERIMENT_ID
    assert decl["wave_classification"] == "NEW_MECHANISM_DEVELOPMENT_STUDY_NON_POINT_IN_TIME_UNIVERSE"
    assert "PROVEN_ALPHA" in decl["forbidden_verdicts"]
    assert "PROMOTION_READY" in decl["forbidden_verdicts"]

    su = decl["seed_universe"]
    assert su["universe_id"] == seed["universe_id"]
    assert su["symbol_count"] == seed["symbol_count"] == len(seed["symbols"])
    assert seed["point_in_time_membership"] is False
    assert su["point_in_time_membership"] is False

    assert decl["data"]["feed"] == FEED == "sip"
    assert decl["data"]["timeframe"] == TIMEFRAME
    assert decl["data"]["start_utc"] == "2016-01-01T00:00:00Z"
    assert decl["data"]["end_utc"] == "2025-05-01T00:00:00Z"
    assert decl["data"]["asof"] == ASOF

    assert decl["label"]["horizon_bars"] == LABEL_HORIZON_BARS
    assert decl["label"]["ret_threshold"] == LABEL_RET_THRESHOLD

    wf = decl["walk_forward"]
    assert wf["train_years"] == WF_TRAIN_YEARS
    assert wf["test_months"] == WF_TEST_MONTHS
    assert wf["step_months"] == WF_STEP_MONTHS
    assert wf["holdout_months"] == WF_HOLDOUT_MONTHS
    assert wf["min_rows_per_fold"] == WF_MIN_ROWS_PER_FOLD
    assert wf["purge_enabled"] is True
    assert wf["embargo_seconds"] == 0

    model = decl["model"]
    assert model["l2"] == MODEL_L2
    assert model["lr"] == MODEL_LR
    assert model["steps"] == MODEL_STEPS
    assert model["standardize"] is MODEL_STANDARDIZE
    assert model["clip_z"] == MODEL_CLIP_Z

    cost = decl["cost_model"]
    assert cost["commission_bps_per_side"] == COMMISSION_BPS_PER_SIDE
    assert cost["slippage_bps_per_side"] == SLIPPAGE_BPS_PER_SIDE

    exe = decl["execution_pricing"]
    assert exe["pricing_model_id"] == EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1
    assert exe["slippage_bps"] == EXECUTION_SLIPPAGE_BPS
    assert exe["volatility_mult_bps"] == EXECUTION_VOLATILITY_MULT_BPS

    wts = decl["weight_to_share"]
    assert wts["equity_usd"] == EQUITY_USD
    assert wts["max_gross_exposure"] == MAX_GROSS_EXPOSURE

    assert decl["rank_side_count"] == RANK_SIDE_COUNT == 5
    assert decl["long_short_min_rankable_symbols"] == 2 * RANK_SIDE_COUNT
    assert decl["borrow_model"] == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    assert decl["boundary_tie_policy"] == "fail_closed_boundary_ties_v1"

    sp = decl["signal_policies"]
    assert sp["rank_long_only"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1
    assert sp["rank_long_only"]["rank_side_count"] == RANK_SIDE_COUNT
    assert sp["rank_long_short"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1
    assert sp["rank_long_short"]["rank_side_count"] == RANK_SIDE_COUNT
    assert sp["rank_long_short"]["borrow_model"] == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1

    assert decl["placebo_seed"] == PLACEBO_SEED

    for key, fam in FAMILIES.items():
        h = decl["hypotheses"][key]
        assert h["feature_columns"] == [fam.feature_column]
        assert h["strategy_id"] == fam.strategy_id
        assert h["hypothesis_id_long_only"] == fam.hyp_long_only
        assert h["hypothesis_id_long_short"] == fam.hyp_long_short
        assert h["hypothesis_id_placebo"] == fam.hyp_placebo

    assert decl["real_candidate_population"] == REAL_CANDIDATE_HYPOTHESIS_IDS
    assert len(REAL_CANDIDATE_HYPOTHESIS_IDS) == 2
    assert decl["diagnostic_placebo_population"] == DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS
    assert len(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS) == 1

    for hyp_id in decl["real_candidate_population"]:
        assert "threshold" not in hyp_id


def signal_policy_for(direction: str) -> SignalPolicySpec:
    if direction == "long_only":
        return SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True,
            rank_side_count=RANK_SIDE_COUNT,
            max_gross_exposure=MAX_GROSS_EXPOSURE,
        )
    if direction == "long_short":
        return SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
            long_only=False,
            rank_side_count=RANK_SIDE_COUNT,
            max_gross_exposure=MAX_GROSS_EXPOSURE,
            # borrow_model left at default -> BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
        )
    raise ValueError(f"unknown direction policy key: {direction!r}")


def economic_fold_date_authority(economic_returns_csv: Path) -> dict:
    """Exact reproduction of SHORT-WAVE-02's own inlined helper (same
    portability convention as build_causal_placebo_targets -- see module
    docstring). Maps every economic-return date to its owning fold and
    records each fold's reset (first) date, failing closed on any date
    mapped to more than one fold.

    R1 (WAVE03-DYNAMIC-BENCHMARK-CAUSALITY-REPAIR-01): also returns
    fold_of_date (date -> fold), needed by build_dynamic_rankable_benchmark's
    causal return attribution to detect (and refuse to bridge) a fold
    boundary when looking up the immediately-prior reference date's
    RANKABLE_SET."""
    econ = pd.read_csv(economic_returns_csv)
    if "fold" not in econ.columns or "timestamp" not in econ.columns:
        raise RuntimeError(f"Fail-closed: {economic_returns_csv} missing required 'fold'/'timestamp' column(s)")
    econ = econ.copy()
    econ["date"] = pd.to_datetime(econ["timestamp"], utc=True).dt.strftime("%Y-%m-%d")
    fold_sets_by_date = econ.groupby("date")["fold"].agg(lambda s: sorted(set(s)))
    ambiguous = {d: fs for d, fs in fold_sets_by_date.items() if len(fs) != 1}
    if ambiguous:
        raise RuntimeError(
            f"Fail-closed: date(s) mapped to more than one economic fold in "
            f"{economic_returns_csv}: {dict(list(ambiguous.items())[:5])}"
        )
    fold_of_date = fold_sets_by_date.map(lambda fs: fs[0])
    reset_dates = set(fold_of_date.reset_index().groupby("fold")["date"].min().tolist())
    return {
        "date_set": set(fold_of_date.index.tolist()),
        "reset_dates": reset_dates,
        "fold_of_date": fold_of_date.to_dict(),
    }


def rankable_set_by_date(oos_predictions_csv: Path) -> dict[str, set[str]]:
    """RANKABLE_SET(T) per calendar date, read directly from one family's
    own test-fold OOS prediction rows (mqk_research.ml.economic_walkforward.
    load_oos_predictions -- the SAME production loader the real economic
    simulator uses to resolve, per exact decision_ts, which symbols were
    actually scored; see _build_rank_pending_events's "DYNAMIC CROSS-SECTION
    V1" docstring). A symbol is rankable on date d iff it has a row whose
    decision_ts falls on d -- membership is exactly whatever this file says
    at that exact row: no carry-forward of a stale prior date's membership,
    no backfilling a later-listed symbol into an earlier date, no synthetic
    default. One family's long-only and long-short real trials share
    byte-identical classification inputs (same features.csv/targets.csv,
    same model) and therefore the same OOS predictions file -- callers pass
    either trial's copy interchangeably.

    Fails closed if the same symbol is scored more than once on the same
    calendar date (would only happen if two folds' test windows overlapped
    on that date, which _parse_used_folds already forbids upstream -- this
    is defense-in-depth on the primitive actually under test here)."""
    oos = load_oos_predictions(oos_predictions_csv)
    oos = oos.copy()
    oos["date"] = oos["decision_ts"].dt.strftime("%Y-%m-%d")
    dup_mask = oos.duplicated(subset=["date", "symbol"], keep=False)
    if dup_mask.any():
        dups = sorted(set(zip(oos.loc[dup_mask, "date"], oos.loc[dup_mask, "symbol"])))
        raise RuntimeError(
            f"Fail-closed: symbol scored more than once on the same calendar date in "
            f"{oos_predictions_csv}: {dups[:5]}"
        )
    by_date: dict[str, set[str]] = {}
    for d, group in oos.groupby("date", sort=True):
        by_date[str(d)] = set(group["symbol"].astype(str))
    return by_date


def build_dynamic_rankable_benchmark(
    bars: pd.DataFrame,
    oos_predictions_csv: Path,
    economic_returns_csv: Path,
    reference_dates: list[str],
    holdout_start_utc: str,
) -> dict:
    """WAVE03-DYNAMIC-RANKABLE-BENCHMARK-01: family-specific dynamic
    equal-weight benchmark (see PREDECLARED_WAVE.json "benchmark"). At each
    economic decision date T, equal-weights EXACTLY RANKABLE_SET(T) --
    the symbols with a causally valid OOS ml_score for this family at that
    exact decision timestamp (rankable_set_by_date, above) -- never a fixed
    universe, never future membership, never a stale carried-forward
    selection. Fold-reset semantics mirror SHORT-WAVE-02's
    build_fold_reset_benchmark convention exactly: each fold's reset date is
    forced to a 0.0 return, and the reference date set must equal the
    economic fold-date authority's own date set exactly (economic_
    fold_date_authority, fail closed on any mismatch).

    EMPTY RANKABLE SET CONTRACT (explicit, documented, tested): a reference
    date whose RANKABLE_SET(T) is empty (or whose rankable symbols have no
    usable daily_ret observation that date) produces NO return observation
    for that date -- it is recorded in `dates_with_no_return_observation`
    and excluded from the cumulative/Sharpe/drawdown computation, exactly
    like SHORT-WAVE-02's own missing-date handling. It is NOT an error by
    itself (a real family can legitimately have zero rankable symbols on a
    date before the underlying model has enough OOS coverage) and it is
    NEVER silently defaulted to a 0.0 return -- only a genuine fold-reset
    date is forced to 0.0, and that is a distinct, unrelated convention
    checked by symbol/date identity, never by emptiness.

    R2 (WAVE03-DYNAMIC-BENCHMARK-FUTURE-EXECUTION-CHRONOLOGY-REPAIR-02)
    FUTURE-EXECUTION CHRONOLOGY -- supersedes R1's single-lag causal shift,
    which still let a decision earn a return one bar too early. The
    production execution contract (execution_rules.md orchestrator phase
    ordering; CLAUDE.md canonical outbox -> broker submit -> broker truth ->
    inbox -> portfolio flow) is: a decision/signal known at reference date D
    can execute only at a strictly later bar; the return interval ENDING at
    that later bar is earned by the position already executed BEFORE it;
    only after that incoming return is realized does the pending decision
    itself execute; the newly executed position can therefore first earn a
    return on the interval AFTER its own execution bar. Modelled here per
    fold as three explicit states advanced date-by-date, in this exact
    order, for every reference date D (including the fold's first date):

      A. RETURN FIRST: the return ending at D (close[D]/close[D_prev]-1) is
         attributed to whatever membership is currently EXECUTED -- i.e.
         the membership some EARLIER decision already had executed BEFORE
         D. A decision recorded at D (or the immediately preceding date)
         must never be allowed to govern this same return -- see the
         PRODUCTION CONTRACT CROSS-CHECK test, which proves the result
         changes if this step is reordered after execution.
      B. EXECUTE PENDING: only now does the pending target membership
         (the decision recorded on D's immediately preceding reference
         date) become the new EXECUTED membership.
      C. RECORD NEW DECISION: RANKABLE_SET(D), read fresh from D's own OOS
         predictions, becomes the new PENDING target for some later date
         to execute.

    Net effect -- the return ending at the reference date two positions
    after D (D's successor's successor) is the first interval RANKABLE_SET
    (D) can ever earn; equivalently, the return ending at reference date
    D_i (i>=2 within a fold) is governed by RANKABLE_SET(D_{i-2}), never by
    D_{i-1}'s or D_i's own decision. EXECUTED starts EMPTY and PENDING
    starts unset at each fold's first date (no fold may seed either from
    the previous fold), so: the fold's first (reset) date's own computed
    return is irrelevant (forced to 0.0 by the pre-existing convention,
    below); the fold's SECOND date is always excluded from
    dates_with_no_return_observation's complement -- i.e. it always
    produces no return observation -- because nothing has executed yet
    regardless of what was decided; only the THIRD date onward can ever
    carry a real observation. This also means a symbol dropped from
    RANKABLE_SET at some date D still earns the D->D_next interval (it was
    already executed before D, from an earlier decision) but never the
    interval after that (its exit itself executes one bar later, per the
    EMPTY RANKABLE SET CONTRACT above whenever the governing decision was
    itself empty).

    HOLDOUT SAFETY: every `reference_dates` entry must fall strictly before
    `holdout_start_utc` (fail closed otherwise, mirroring SHORT-WAVE-02); the
    function only ever reads `bars`/`oos_predictions_csv` rows whose date is
    in `reference_dates` (T_prev is itself always an element of
    `reference_dates`), so a holdout-region row is structurally never
    touched even though `bars` and the OOS predictions file may span dates
    beyond it.

    R3 (WAVE03-BENCHMARK-MISSING-BAR-EXECUTION-AUTHORITY-REPAIR-03) REAL-BAR
    AUTHORITY: a non-empty EXECUTED basket may never silently shrink to its
    surviving members -- every EXECUTED symbol must have an exact close at
    both the previous reference date and the current one, or the return
    computation raises RuntimeError (never drops/renormalizes/NaNs-through).
    Every membership transition (entry or exit, i.e. EXECUTED ^ PENDING)
    also requires a real execution bar AT the execution date itself, or step
    B raises RuntimeError rather than fabricate an execution. These checks
    key strictly off reference dates via an exact (symbol, date) close
    lookup -- never off a bar row's previous OBSERVED date -- so a missing
    reference-date bar can never be silently bridged into a longer return."""
    ref_ts = pd.to_datetime(pd.Index(reference_dates), utc=True)
    holdout_ts = pd.Timestamp(holdout_start_utc)
    if holdout_ts.tzinfo is None:
        holdout_ts = holdout_ts.tz_localize("UTC")
    if (ref_ts >= holdout_ts).any():
        bad = sorted(str(d) for d in ref_ts[ref_ts >= holdout_ts])
        raise RuntimeError(
            f"Fail-closed: {len(bad)} benchmark reference date(s) fall at/after the reserved holdout "
            f"boundary ({holdout_ts.isoformat()}): {bad[:5]}{'...' if len(bad) > 5 else ''}"
        )

    authority = economic_fold_date_authority(economic_returns_csv)
    economic_date_set = authority["date_set"]
    fold_start_dates = authority["reset_dates"]
    fold_of_date = authority["fold_of_date"]
    reference_date_set = set(pd.Index(reference_dates).astype(str))
    if reference_date_set != economic_date_set:
        only_reference = sorted(reference_date_set - economic_date_set)
        only_economic = sorted(economic_date_set - reference_date_set)
        raise RuntimeError(
            f"Fail-closed: reference date(s) and economic fold-authority date(s) in "
            f"{economic_returns_csv} are not identical -- "
            f"reference_only={only_reference[:5]} economic_only={only_economic[:5]}"
        )

    rankable = rankable_set_by_date(oos_predictions_csv)

    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")

    # R3 (WAVE03-BENCHMARK-MISSING-BAR-EXECUTION-AUTHORITY-REPAIR-03): exact
    # (symbol, reference-date) -> close lookup, replacing the prior
    # groupby(...).pct_change() authority. pct_change() walked each symbol's
    # own previous OBSERVED bar row, which need not be the previous economic
    # REFERENCE date -- a missing row could silently bridge a gap (e.g.
    # D1->D3 read as a single-interval return when D2 has no bar at all).
    # Every return/execution lookup below is keyed by the exact reference
    # date string, never by row adjacency. Non-finite/non-positive closes are
    # treated as absent (fail closed, never used as a causal endpoint).
    close_lookup: dict[tuple[str, str], float] = {}
    for sym, date, close in zip(b["symbol"].astype(str), b["date"], b["close"]):
        if pd.notna(close) and np.isfinite(close) and close > 0:
            close_lookup[(sym, date)] = float(close)

    sorted_dates = sorted(reference_date_set)
    # SIGNAL-AVAILABILITY membership (unshifted -- how many symbols were
    # actually scored AT date T; feeds rankable_cross_section_targets
    # elsewhere) is deliberately distinct from the CAUSAL RETURN attribution
    # below (which uses T_prev's membership) -- see docstring.
    cross_section_size: dict[str, int] = {d: len(rankable.get(d, set())) for d in sorted_dates}

    # Reference dates partition into contiguous per-fold blocks when sorted
    # (fold_start_dates marks each block's first entry). Group into those
    # blocks and run the DECISION/PENDING/EXECUTED state machine
    # independently per fold -- EXECUTED and PENDING must never be seeded
    # from a prior fold's state (FOLD BOUNDARY requirement).
    fold_blocks: list[list[str]] = []
    current_fold_id: Any = object()
    for d in sorted_dates:
        fid = fold_of_date.get(d)
        if fid != current_fold_id:
            fold_blocks.append([])
            current_fold_id = fid
        fold_blocks[-1].append(d)

    per_date: dict[str, float] = {}
    for block in fold_blocks:
        executed: set[str] = set()
        pending: Optional[set[str]] = None
        for idx, d in enumerate(block):
            # A. RETURN FIRST -- attribute D's incoming return to whatever
            # is currently EXECUTED (set by an earlier date's step B, never
            # by this date's own decision below). Empty EXECUTED covers both
            # a fold still bootstrapping (nothing executed yet) and a
            # genuinely empty governing decision -- both are legitimately
            # missing observations, never a fabricated 0.0.
            if not executed:
                per_date[d] = float("nan")
            else:
                # d_prev is always defined here: EXECUTED can only be
                # non-empty from the fold's 3rd date onward (it is first set
                # by step B on the fold's 2nd iteration), so idx >= 2.
                d_prev = block[idx - 1]
                missing_endpoints: list[str] = []
                r_values: list[float] = []
                for s in sorted(executed):
                    c_prev = close_lookup.get((s, d_prev))
                    c_cur = close_lookup.get((s, d))
                    if c_prev is None or c_cur is None:
                        missing_endpoints.append(s)
                        continue
                    r_values.append(c_cur / c_prev - 1.0)
                if missing_endpoints:
                    # DEFECT A/B: a non-empty EXECUTED basket must never
                    # silently shrink to its surviving members -- every
                    # member must have an exact close at both D_prev and D.
                    raise RuntimeError(
                        f"Fail-closed: EXECUTED symbol(s) {missing_endpoints} lack an exact causal "
                        f"close at both the previous reference date {d_prev!r} and reference date "
                        f"{d!r}; a partial basket may never be silently used for the benchmark return"
                    )
                per_date[d] = float(np.mean(r_values))
            # B. EXECUTE PENDING STATE -- only after D's incoming return is
            # already attributed above does the pending target (decided on
            # D's immediately preceding reference date) become EXECUTED.
            if pending is not None:
                # DEFECT C: a membership transition (entry or exit) may only
                # be applied if every transition symbol has a real execution
                # bar AT D itself -- otherwise this benchmark would be
                # fabricating an execution that never truthfully occurred.
                transition_symbols = executed.symmetric_difference(pending)
                missing_execution_bar = sorted(s for s in transition_symbols if close_lookup.get((s, d)) is None)
                if missing_execution_bar:
                    raise RuntimeError(
                        f"Fail-closed: membership transition symbol(s) {missing_execution_bar} lack a "
                        f"real execution bar at reference date {d!r}; cannot apply PENDING without a "
                        f"truthful execution price"
                    )
                executed = pending
            # C. RECORD NEW DECISION -- read RANKABLE_SET(d) fresh; this
            # becomes the PENDING target for some later date to execute.
            pending = rankable.get(d, set())

    per_date_series = pd.Series(per_date).reindex(sorted_dates)
    for d in fold_start_dates & reference_date_set:
        per_date_series.loc[d] = 0.0

    missing_dates = sorted(d for d in sorted_dates if pd.isna(per_date_series.loc[d]))
    daily_series = per_date_series.dropna()
    cumulative_return = float(np.prod(1.0 + daily_series.to_numpy()) - 1.0) if len(daily_series) else None
    sharpe = compute_sharpe(daily_series) if len(daily_series) else None
    max_drawdown = compute_max_drawdown(daily_series) if len(daily_series) else None

    sizes = list(cross_section_size.values())
    return {
        "benchmark_type": "dynamic_equal_weight_causally_rankable_fold_reset_v1",
        "reference_date_count": len(reference_dates),
        "reference_date_start": str(min(reference_dates)),
        "reference_date_end": str(max(reference_dates)),
        "fold_reset_dates_count": len(fold_start_dates & reference_date_set),
        "rankable_cross_section_size_by_date": cross_section_size,
        "rankable_cross_section_min": min(sizes) if sizes else 0,
        "rankable_cross_section_median": float(np.median(sizes)) if sizes else None,
        "rankable_cross_section_max": max(sizes) if sizes else 0,
        "dates_with_zero_rankable_symbols": sorted(d for d, n in cross_section_size.items() if n == 0),
        "dates_with_no_return_observation": missing_dates,
        "daily_return_observations_used": int(len(daily_series)),
        "cumulative_return_over_reference_dates": cumulative_return,
        "sharpe": sharpe,
        "max_drawdown": max_drawdown,
        "holdout_start_utc": holdout_ts.isoformat(),
    }


# ---------------------------------------------------------------------------
# Network-touching stages (never reached without --execute)
# ---------------------------------------------------------------------------


def _load_paper_credentials_into_env() -> None:
    """Read ONLY ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER from the
    frozen Paper repo's .env.local into process memory if not already
    present. Never prints values. Never reads Live credentials. Never
    modifies .env.local. ONLY ever called from ensure_bars(), which is
    only reachable from an --execute-gated stage -- see main()."""
    required = ("ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER")
    if all(os.environ.get(k) for k in required):
        return
    env_path = PRIMARY_PAPER_REPO / ".env.local"
    if not env_path.exists():
        raise RuntimeError(f"Fail-closed: no credentials in process env and {env_path} does not exist")
    for line in env_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        if line.startswith("export "):
            line = line[len("export "):]
        key, _, value = line.partition("=")
        key = key.strip()
        if key not in required:
            continue
        value = value.strip().strip('"').strip("'")
        os.environ.setdefault(key, value)
    missing = [k for k in required if not os.environ.get(k)]
    if missing:
        raise RuntimeError(f"Fail-closed: required credential(s) not found in {env_path}: {missing}")


def verify_wave03_bars_cache_authority(bars: pd.DataFrame, manifest: dict) -> None:
    """R2 (WAVE03-CACHE-AUTHORITY-REPAIR-01): fail-closed verification that a
    persisted raw_bars.csv/bars_provenance_manifest.json pair actually
    describes THIS Wave-03 run's frozen identity (PREDECLARED_WAVE.json
    "cache_safety": provider, feed, adjustment, asof, requested window,
    frozen seed universe, corporate-action evidence, canonical semantic
    bars hash) before any derived artifact (targets/features) may be
    computed from it. Reuses existing production verification seams
    (mqk_research.data.bars_provenance) rather than inventing a second
    competing bars hash:
      - require_registered_bars_provenance: manifest-SHAPE structural gate
        (known price convention, real CA policy, supported universe mode).
      - require_bars_match_manifest: CONTENT-BINDING preflight -- the bars
        actually on disk recompute to the manifest's own declared canonical
        semantic hash, symbol universe, and timestamp range (catches a
        stale/wrong manifest paired with different bars).
      - check_corporate_action_integrity: the manifest's adjusted-data /
        forbid-affected-periods claim is independently verified, not merely
        asserted.
    Then binds the verified manifest to Wave-03's OWN frozen identity
    (timeframe/window/seed-universe/feed/asof) -- the three checks above
    only prove internal manifest/bars consistency, not that this is the
    RIGHT extraction for this wave. Raises (never silently reuses) on any
    mismatch -- no existence-only cache reuse."""
    require_registered_bars_provenance(manifest)
    require_bars_match_manifest(bars, manifest)
    check_corporate_action_integrity(bars, manifest)

    manifest_timeframe = manifest.get("timeframe")
    if manifest_timeframe != TIMEFRAME:
        raise RuntimeError(
            f"Fail-closed: cached Wave-03 bars manifest timeframe={manifest_timeframe!r} != frozen "
            f"timeframe {TIMEFRAME!r}"
        )
    manifest_start = pd.Timestamp(manifest.get("start_utc"))
    manifest_end = pd.Timestamp(manifest.get("end_utc"))
    if manifest_start != START_UTC or manifest_end != END_UTC:
        raise RuntimeError(
            f"Fail-closed: cached Wave-03 bars manifest window [{manifest_start}, {manifest_end}) != "
            f"frozen window [{START_UTC}, {END_UTC})"
        )

    expected_symbols = sorted({str(s).strip().upper() for s in seed_symbols()})
    actual_symbols = list(manifest.get("symbol_universe") or [])
    if actual_symbols != expected_symbols:
        raise RuntimeError(
            "Fail-closed: cached Wave-03 bars manifest symbol_universe does not equal the frozen "
            f"seed universe (manifest has {len(actual_symbols)} symbols, frozen seed has "
            f"{len(expected_symbols)})"
        )

    attestation = manifest.get("source_attestation") or {}
    if attestation.get("feed") != FEED:
        raise RuntimeError(
            f"Fail-closed: cached Wave-03 bars source_attestation feed={attestation.get('feed')!r} != "
            f"frozen feed {FEED!r}"
        )
    if attestation.get("asof") != ASOF:
        raise RuntimeError(
            f"Fail-closed: cached Wave-03 bars source_attestation asof={attestation.get('asof')!r} != "
            f"frozen asof {ASOF!r}"
        )


def ensure_bars() -> tuple[pd.DataFrame, dict]:
    """Fetch, or reuse this run's own prior fetch of, real Alpaca bars,
    feed=sip, for the frozen seed universe/window/asof. R2
    (WAVE03-CACHE-AUTHORITY-REPAIR-01): a persisted raw_bars.csv/
    bars_provenance_manifest.json pair is reused ONLY after
    verify_wave03_bars_cache_authority passes fail-closed verification --
    never on existence alone. A partial cache (one file present, the other
    missing) fails closed rather than being silently treated as either
    'no cache' or 'valid cache'."""
    bars_path = RUN_ROOT / "raw_bars.csv"
    manifest_path = RUN_ROOT / "bars_provenance_manifest.json"
    bars_present = bars_path.exists()
    manifest_present = manifest_path.exists()
    if bars_present != manifest_present:
        raise RuntimeError(
            "Fail-closed: orphan Wave-03 bars cache artifact -- "
            f"{bars_path.name} present={bars_present}, {manifest_path.name} present={manifest_present}; "
            "refusing to reuse a partial cache"
        )

    if bars_present and manifest_present:
        on_disk_bars = pd.read_csv(bars_path)
        on_disk_manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        verify_wave03_bars_cache_authority(on_disk_bars, on_disk_manifest)
        return on_disk_bars, on_disk_manifest

    from mqk_research.data.alpaca_historical import extract_research_bars_with_provenance

    _load_paper_credentials_into_env()
    result = extract_research_bars_with_provenance(
        symbols=seed_symbols(), start_utc=START_UTC, end_utc=END_UTC,
        timeframe=TIMEFRAME, asof=ASOF, feed=FEED,
    )
    verify_wave03_bars_cache_authority(result["bars"], result["manifest"])
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    result["bars"].to_csv(bars_path, index=False)
    manifest_path.write_text(json.dumps(result["manifest"], sort_keys=True, indent=2, default=str), encoding="utf-8")
    return result["bars"], result["manifest"]


def build_targets(bars: pd.DataFrame, *, horizon_bars: int, ret_threshold: float) -> pd.DataFrame:
    """Exact reproduction of SHORT-WAVE-02's own inlined helper (same
    portability convention as build_causal_placebo_targets). fwd_ret =
    log(close[t+horizon]/close[t]); label_end_ts is the truthful inclusive
    timestamp of the bar whose close completes the label. Classification
    LABEL only -- never executable P&L."""
    bars = bars.copy()
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True)
    bars = bars.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    rows = []
    for sym, g in bars.groupby("symbol", sort=True):
        g = g.reset_index(drop=True)
        closes = g["close"].astype(np.float64).to_numpy()
        ts_list = g["end_ts"].to_numpy()
        n = len(g)
        for j in range(n - horizon_bars):
            k = j + horizon_bars
            c0, c1 = float(closes[j]), float(closes[k])
            if c0 <= 0.0 or c1 <= 0.0:
                continue
            fwd_ret = float(np.log(c1 / c0))
            rows.append(
                {
                    "symbol": sym,
                    "end_ts": str(pd.Timestamp(ts_list[j])),
                    "fwd_ret": fwd_ret,
                    "target": 1 if fwd_ret > ret_threshold else 0,
                    "label_end_ts": pd.Timestamp(ts_list[k]).isoformat(),
                }
            )
    out = pd.DataFrame(rows)
    if out.empty:
        raise RuntimeError("No labeled rows produced -- check symbol/date coverage")
    return out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)


def ensure_real_targets(bars: pd.DataFrame) -> pd.DataFrame:
    """R2 (WAVE03-CACHE-AUTHORITY-REPAIR-01): deliberately NOT
    existence-cached -- unlike bars (which carry an independently
    verifiable provenance manifest), no equivalently strong content-binding
    exists for a bare targets.csv on disk, so the fastest safe design is to
    always recompute deterministically from the already-verified in-memory
    `bars` this call was given (never re-reading a possibly-stale file from
    a prior/different bars snapshot). Still written to disk for audit."""
    targets = build_targets(bars, horizon_bars=LABEL_HORIZON_BARS, ret_threshold=LABEL_RET_THRESHOLD)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    targets.to_csv(RUN_ROOT / "real_targets.csv", index=False)
    return targets


def ensure_placebo_targets(real_targets: pd.DataFrame) -> pd.DataFrame:
    """R2: see ensure_real_targets -- always recomputed from the
    already-verified `real_targets` this call was given, never
    existence-cached."""
    placebo = build_causal_placebo_targets(real_targets, seed=PLACEBO_SEED)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    placebo.to_csv(RUN_ROOT / "placebo_targets.csv", index=False)
    return placebo


def ensure_full_features(bars: pd.DataFrame) -> pd.DataFrame:
    """DEFAULT FeatureSetV1Spec already produces ret_rank_20 (default
    cross_section_windows=(5,20)), ret_5 (default ret_windows includes 5),
    and gap_pct_1 (always computed) -- no spec customization required (see
    SHORT-WAVE-02's identical observation). R2: see ensure_real_targets --
    always recomputed from the already-verified in-memory `bars`, never
    existence-cached."""
    feats = build_feature_set_v1(bars)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    feats.to_csv(RUN_ROOT / "full_features.csv", index=False)
    return feats


def isolate_feature(features: pd.DataFrame, feature_column: str) -> pd.DataFrame:
    """FEATURE ISOLATION INVARIANT: select ONLY symbol, end_ts, and the one
    declared feature column from the full FeatureSetV1 output -- guards
    against the historical ALPHA-01 full-feature-matrix-consumption defect
    (see test_wave03_family_harness.py)."""
    missing = [c for c in ("symbol", "end_ts", feature_column) if c not in features.columns]
    if missing:
        raise RuntimeError(f"feature isolation failed: FeatureSetV1 output missing required column(s) {missing}")
    return features[["symbol", "end_ts", feature_column]].copy()


def assert_single_feature_schema(schema_path: Path, expected_column: str) -> None:
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    actual = schema.get("feature_columns")
    if actual != [expected_column]:
        raise RuntimeError(
            f"FEATURE ISOLATION INVARIANT VIOLATED: expected feature_columns == "
            f"{[expected_column]!r}, got {actual!r} (schema={schema_path})"
        )


def write_run_dir(run_dir: Path, bars: pd.DataFrame, isolated_features: pd.DataFrame,
                   targets: pd.DataFrame, *, feature_column: str) -> Path:
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_path = run_dir / "bars.csv"
    bars_out = bars.copy()
    bars_out["end_ts"] = pd.to_datetime(bars_out["end_ts"], utc=True).map(lambda t: t.isoformat())
    bars_out.to_csv(bars_path, index=False)

    feat_clean = isolated_features.dropna(axis=0, how="any")
    common_keys = feat_clean[["symbol", "end_ts"]].merge(
        targets[["symbol", "end_ts"]], on=["symbol", "end_ts"], how="inner"
    )
    if common_keys.empty:
        raise RuntimeError(
            f"No overlapping non-NaN (symbol,end_ts) rows between isolated features "
            f"({len(feat_clean)} after dropna, {len(isolated_features)} before) and targets ({len(targets)})"
        )
    feat_out = feat_clean.merge(common_keys, on=["symbol", "end_ts"], how="inner")
    targ_out = targets.merge(common_keys, on=["symbol", "end_ts"], how="inner")

    feat_out = feat_out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    targ_out = targ_out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    feat_out.to_csv(run_dir / "features.csv", index=False)
    targ_out.to_csv(run_dir / "targets.csv", index=False)
    schema_path = generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])
    assert_single_feature_schema(schema_path, feature_column)
    return bars_path


def run_one_trial(*, run_dir: Path, experiment_id: str, hypothesis_id: str, strategy_id: str,
                   direction: str, bars_path: Path, bars_provenance: dict) -> dict:
    wf_spec = WalkForwardSpec(
        train_years=WF_TRAIN_YEARS,
        test_months=WF_TEST_MONTHS,
        step_months=WF_STEP_MONTHS,
        holdout_months=WF_HOLDOUT_MONTHS,
        min_rows_per_fold=WF_MIN_ROWS_PER_FOLD,
        purge_enabled=True,
        embargo_seconds=0,
    )
    economic_spec = EconomicWalkForwardSpec(
        signal_policy=signal_policy_for(direction),
        cost_model=CostModelSpec(
            commission_bps_per_side=COMMISSION_BPS_PER_SIDE, slippage_bps_per_side=SLIPPAGE_BPS_PER_SIDE
        ),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=EXECUTION_SLIPPAGE_BPS,
            volatility_mult_bps=EXECUTION_VOLATILITY_MULT_BPS,
        ),
        weight_to_share=WeightToShareSpec(equity_usd=EQUITY_USD),
        annualization=AnnualizationSpec(),
    )

    economic_out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        bars_csv=bars_path,
        economic_spec=economic_spec,
        bars_provenance=bars_provenance,
        registry_db=REGISTRY_DB,
        wf_spec=wf_spec,
        l2=MODEL_L2,
        lr=MODEL_LR,
        steps=MODEL_STEPS,
        standardize=MODEL_STANDARDIZE,
        clip_z=MODEL_CLIP_Z,
    )
    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))
    aggregate = economic_out.get("aggregate")
    if aggregate is None:
        raise RuntimeError(
            f"Fail-closed: economic_walk_forward.json missing required 'aggregate' block: {economic_out_path}"
        )
    holdout = economic_out.get("holdout")
    if not isinstance(holdout, dict) or holdout.get("status") != RESERVED_NOT_EVALUATED_HOLDOUT_STATUS:
        raise RuntimeError(
            "Fail-closed (R4 WAVE03-RUN-RECORDING-TRUTH-REPAIR-01): expected the registered economic "
            f"evaluator's own holdout.status == {RESERVED_NOT_EVALUATED_HOLDOUT_STATUS!r} for "
            f"hypothesis_id={hypothesis_id!r}, got {holdout!r} ({economic_out_path}) -- refusing to "
            "record a family result whose final holdout may already be consumed/evaluated"
        )
    inputs = economic_out.get("inputs", {})
    oos_predictions_csv = inputs.get("oos_predictions_csv", {}).get("path")
    wf_eval_ref = inputs.get("walk_forward_eval", {}).get("path")
    holdout_start_utc = None
    folds_generated = folds_used = folds_skipped = None
    if wf_eval_ref:
        wf_eval = json.loads(Path(wf_eval_ref).read_text(encoding="utf-8"))
        holdout_start_utc = wf_eval.get("temporal_contract", {}).get("holdout_start_utc")
        summary = wf_eval.get("summary", {})
        folds_generated = summary.get("folds_total")
        folds_used = summary.get("folds_used")
        if folds_generated is not None and folds_used is not None:
            folds_skipped = folds_generated - folds_used

    return {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "direction": direction,
        "trial_id": economic_out["registry"]["trial_id"],
        "economic_eval_id": economic_out["ids"]["economic_eval_id"],
        "economic_walk_forward_json": str(economic_out_path),
        "economic_daily_returns_csv": economic_out.get("outputs", {}).get("economic_daily_returns_csv", {}).get("path"),
        "economic_returns_csv": economic_out.get("outputs", {}).get("economic_returns_csv", {}).get("path"),
        "oos_predictions_csv": oos_predictions_csv,
        "aggregate": aggregate,
        "folds": economic_out.get("folds", []),
        "holdout": economic_out.get("holdout"),
        "holdout_start_utc": holdout_start_utc,
        "folds_generated": folds_generated,
        "folds_used": folds_used,
        "folds_skipped": folds_skipped,
    }


# ---------------------------------------------------------------------------
# Required output recording fields (PREDECLARED_WAVE.json
# "required_future_run_recording_fields") -- every helper below derives its
# field(s) truthfully from an already-persisted, already-accepted-engine
# artifact (economic_walk_forward.json's aggregate/folds/weight_to_share_
# evidence, economic_returns.csv's fold/date authority, the family's own OOS
# predictions, bars.csv) or from the frozen predeclaration's own config
# constants -- never fabricated, never re-derived by reimplementing the
# accepted engine's own signal-resolution logic.
# ---------------------------------------------------------------------------


def rankable_cross_section_targets(
    cross_section_size_by_date: dict[str, int], *, rank_side_count: int, long_only: bool
) -> dict:
    """DESIRED (signal-time) symbol-day counts for one direction, derived
    directly from build_dynamic_rankable_benchmark's own per-date cross-
    section sizes: the frozen cross-sectional rank policy is a FIXED-K
    selection (exactly rank_side_count longs, and for long_short exactly
    rank_side_count more shorts) that either fully succeeds or fails closed
    entirely for a given date's frame (_resolve_rank_direction_for_frame) --
    so the desired long/short count on any date with a sufficient cross-
    section is deterministically rank_side_count, and 0 on any date below
    the minimum. No per-symbol identity is needed for this count, only the
    cross-section size already computed by Patch A -- reuses that output
    rather than re-invoking the private rank-resolution primitive."""
    min_required = rank_side_count if long_only else 2 * rank_side_count
    target_long_days = 0
    target_short_days = 0
    dates_below_minimum = []
    for d, n in cross_section_size_by_date.items():
        if n < min_required:
            dates_below_minimum.append(d)
            continue
        target_long_days += rank_side_count
        if not long_only:
            target_short_days += rank_side_count
    return {
        "target_long_symbol_days": target_long_days,
        "target_short_symbol_days": target_short_days,
        "dates_below_rank_minimum": sorted(dates_below_minimum),
        "max_concurrent_target_longs": rank_side_count if target_long_days else 0,
        "max_concurrent_target_shorts": rank_side_count if (not long_only and target_short_days) else 0,
    }


def symbols_ever_never_rankable(rankable_by_date: dict[str, set[str]], universe: list[str]) -> dict:
    ever: set[str] = set()
    for syms in rankable_by_date.values():
        ever |= syms
    return {
        "symbols_ever_rankable": sorted(ever),
        "symbols_never_rankable": sorted(set(universe) - ever),
    }


def first_last_rankable_date_per_symbol(rankable_by_date: dict[str, set[str]]) -> dict:
    first: dict[str, str] = {}
    last: dict[str, str] = {}
    for d in sorted(rankable_by_date.keys()):
        for sym in rankable_by_date[d]:
            if sym not in first:
                first[sym] = d
            last[sym] = d
    return {sym: {"first": first[sym], "last": last[sym]} for sym in sorted(first)}


def _fold_date_index(economic_returns_csv: Path) -> dict[int, list[str]]:
    """fold -> sorted calendar dates that fold's economic_returns.csv rows
    cover -- used only to forward-fill weight_to_share_evidence's sparse
    per-symbol change events onto a full daily index for EXECUTED symbol-
    day/exposure accounting; never used for return computation itself."""
    econ = pd.read_csv(economic_returns_csv)
    econ = econ.copy()
    econ["date"] = pd.to_datetime(econ["timestamp"], utc=True).dt.strftime("%Y-%m-%d")
    out: dict[int, list[str]] = {}
    for fold, g in econ.groupby("fold"):
        out[int(fold)] = sorted(g["date"].unique())
    return out


def reconstruct_daily_target_qty(fold_summaries: list[dict], economic_returns_csv: Path) -> pd.DataFrame:
    """Per (fold, symbol, date) EXECUTED signed share position, forward-
    filled from each fold summary's weight_to_share_evidence (already-
    persisted, already-accepted P7B-REPAIR-01/02 evidence: target_qty is the
    RESULTING position after that row's fill-time admit/reject decision,
    signed positive=long/negative=short/zero=flat -- weight_to_target_qty's
    own documented sign convention) onto the fold's own full trading-day
    index. Never carried across a fold boundary -- _simulate_fold force-
    flattens every symbol at fold end, so each fold's reconstruction starts
    fresh. Adds no new signal-resolution logic -- purely a forward-fill
    projection of already-computed, already-persisted evidence."""
    fold_dates = _fold_date_index(economic_returns_csv)
    rows = []
    for fs in fold_summaries:
        fold_no = int(fs["fold"])
        dates = fold_dates.get(fold_no, [])
        if not dates:
            continue
        evidence = fs.get("weight_to_share_evidence") or {}
        date_index = pd.Index(dates)
        for sym, events in evidence.items():
            if not events:
                continue
            ev = pd.DataFrame(events)
            ev["date"] = pd.to_datetime(ev["timestamp"], utc=True).dt.strftime("%Y-%m-%d")
            ev = ev.sort_values("date", kind="mergesort").drop_duplicates("date", keep="last")
            qty_by_date = ev.set_index("date")["target_qty"].reindex(date_index)
            qty_by_date = qty_by_date.ffill().fillna(0).astype(int)
            for d, qty in zip(date_index, qty_by_date):
                rows.append({"fold": fold_no, "symbol": sym, "date": d, "target_qty": int(qty)})
    if not rows:
        return pd.DataFrame(columns=["fold", "symbol", "date", "target_qty"])
    return pd.DataFrame(rows)


def executed_symbol_day_stats(daily_positions: pd.DataFrame) -> dict:
    if daily_positions.empty:
        return {
            "executed_long_symbol_days": 0, "executed_short_symbol_days": 0,
            "max_concurrent_longs": 0, "max_concurrent_shorts": 0,
        }
    long_mask = daily_positions["target_qty"] > 0
    short_mask = daily_positions["target_qty"] < 0
    per_date_long = daily_positions.loc[long_mask].groupby("date").size()
    per_date_short = daily_positions.loc[short_mask].groupby("date").size()
    return {
        "executed_long_symbol_days": int(long_mask.sum()),
        "executed_short_symbol_days": int(short_mask.sum()),
        "max_concurrent_longs": int(per_date_long.max()) if len(per_date_long) else 0,
        "max_concurrent_shorts": int(per_date_short.max()) if len(per_date_short) else 0,
    }


class MissingCausalMarkError(RuntimeError):
    """Fail-closed (R4 WAVE03-RUN-RECORDING-TRUTH-REPAIR-01): raised when a
    nonzero held position has no causal mark (current-or-prior bar close)
    available at all -- never silently dropped from exposure."""


def price_held_positions_causally(daily_positions: pd.DataFrame, bars: pd.DataFrame) -> pd.DataFrame:
    """Attach a CAUSAL mark price to each (fold, symbol, date) held-position
    row: today's close when the symbol has a bar that date, otherwise the
    most recent PRIOR bar's close for that symbol (last known causal close
    -- NEVER a future price), reconstructed independently per (fold, symbol)
    -- daily_positions is already fold-scoped by reconstruct_daily_target_qty
    (which never forward-fills a position across a fold boundary), so no
    additional fold-scoping of the price lookback is needed here to satisfy
    "do not carry POSITION state across fold boundaries"; using a real prior
    close from before the fold started is legitimate historical pricing, not
    carried position state. FAILS CLOSED (MissingCausalMarkError) if a
    nonzero-qty row has no causal mark available at all -- never silently
    dropped from exposure."""
    if daily_positions.empty:
        return daily_positions.assign(close=pd.Series(dtype=float))

    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort")
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")
    b = b.drop_duplicates(subset=["symbol", "date"], keep="last")
    b["_date_ts"] = pd.to_datetime(b["date"])

    out_frames = []
    for (_fold, sym), grp in daily_positions.groupby(["fold", "symbol"], sort=False):
        grp = grp.sort_values("date", kind="mergesort").copy()
        grp["_date_ts"] = pd.to_datetime(grp["date"])
        sym_bars = b.loc[b["symbol"] == sym, ["_date_ts", "close"]].sort_values("_date_ts", kind="mergesort")
        if sym_bars.empty:
            # merge_asof rejects an empty right frame's default dtype against
            # a non-empty left frame's datetime dtype -- and there is no
            # causal mark to find here regardless, so skip straight to NaN.
            grp["close"] = np.nan
            out_frames.append(grp.drop(columns=["_date_ts"]))
            continue
        merged = pd.merge_asof(grp, sym_bars, on="_date_ts", direction="backward")
        out_frames.append(merged.drop(columns=["_date_ts"]))
    result = pd.concat(out_frames, ignore_index=True)

    missing_mask = (result["target_qty"] != 0) & result["close"].isna()
    if bool(missing_mask.any()):
        bad = result.loc[missing_mask, ["fold", "symbol", "date", "target_qty"]].to_dict("records")
        raise MissingCausalMarkError(
            f"Fail-closed: no causal mark (current-or-prior bar) available for {len(bad)} nonzero "
            f"held position row(s) -- refusing to silently drop them from exposure: {bad[:5]}"
        )
    return result


def actual_gross_and_net_exposure_from_positions(
    daily_positions: pd.DataFrame, bars: pd.DataFrame, equity_usd: float
) -> tuple[Optional[float], Optional[float]]:
    """Reconstructed EXECUTED discrete gross/net exposure -- the counterpart
    to the accepted engine's own persisted aggregate.average_gross_exposure
    (a CONTINUOUS desired-weight series), required because a continuous
    nonzero desired weight can resolve to a discrete target_qty=0 after
    fill-time admit/reject rounding/caps (see weight_to_share). For each
    (fold, symbol, date) row: weight = target_qty * causal_mark(symbol,
    date) / equity_usd (mirrors the accepted engine's own P7B-REPAIR-02
    dollar-ledger weight formula, but priced via price_held_positions_
    causally, which never future-fills and never silently drops a held
    position for lacking today's bar). gross = mean over dates of
    sum(abs(weight)); net = mean over dates of sum(weight). Flat
    (target_qty=0) rows contribute 0 regardless of their mark price, so
    they never need a resolvable price."""
    if daily_positions.empty:
        return None, None
    priced = price_held_positions_causally(daily_positions, bars)
    priced = priced.copy()
    priced["weight"] = priced["target_qty"].astype(float) * priced["close"].fillna(0.0).astype(float) / float(equity_usd)
    per_date_net = priced.groupby("date")["weight"].sum()
    per_date_gross = priced.groupby("date")["weight"].apply(lambda w: w.abs().sum())
    gross = float(per_date_gross.mean()) if len(per_date_gross) else None
    net = float(per_date_net.mean()) if len(per_date_net) else None
    return gross, net


def fold_concentration(fold_summaries: list[dict]) -> Optional[float]:
    """Fraction of total ABSOLUTE per-fold net_total_return contributed by
    the single most-concentrated fold -- a simple, documented, auditable
    concentration ratio derived from already-persisted per-fold aggregate
    data (economic_walk_forward.json's "folds" array). None when there are
    no folds or every fold's net_total_return is exactly zero (undefined
    concentration -- not fabricated as 0.0 or 1.0)."""
    values = [abs(float(fs["net_total_return"])) for fs in fold_summaries if fs.get("net_total_return") is not None]
    total = sum(values)
    if not values or total <= 0.0:
        return None
    return float(max(values) / total)


def compute_paired_delta(long_only: dict, long_short: dict) -> dict:
    """delta = long_short - long_only, over exact matching OOS dates
    (exact reproduction of SHORT-WAVE-02's own inlined helper)."""
    lo_daily = pd.read_csv(long_only["economic_daily_returns_csv"])
    ls_daily = pd.read_csv(long_short["economic_daily_returns_csv"])
    lo_turnover = float(lo_daily["turnover"].sum())
    ls_turnover = float(ls_daily["turnover"].sum())

    lo_agg = long_only["aggregate"]
    ls_agg = long_short["aggregate"]

    def _delta(key):
        a, b = lo_agg.get(key), ls_agg.get(key)
        if a is None or b is None:
            return None
        return float(b) - float(a)

    return {
        "delta_net_total_return": _delta("net_total_return"),
        "delta_net_sharpe": _delta("net_sharpe"),
        "delta_max_drawdown": _delta("max_drawdown"),
        "delta_turnover": ls_turnover - lo_turnover,
        "delta_cost_drag": _delta("cost_drag"),
        "long_only_turnover": lo_turnover,
        "long_short_turnover": ls_turnover,
    }


def compute_placebo_delta(long_short: dict, placebo: dict) -> dict:
    """delta = long_short(real) - placebo, over exact matching OOS dates
    (exact reproduction of SHORT-WAVE-02's own inlined helper)."""
    ls_agg = long_short["aggregate"]
    pb_agg = placebo["aggregate"]

    def _delta(key):
        a, b = pb_agg.get(key), ls_agg.get(key)
        if a is None or b is None:
            return None
        return float(b) - float(a)

    return {
        "delta_net_total_return": _delta("net_total_return"),
        "delta_net_sharpe": _delta("net_sharpe"),
        "delta_max_drawdown": _delta("max_drawdown"),
    }


def compute_family_recording_fields(
    fam: HypothesisFamily, r_lo: dict, r_ls: dict, r_pb: dict,
    benchmark_lo: dict, benchmark_ls: dict, rankable_by_date: dict[str, set[str]], bars: pd.DataFrame,
) -> dict:
    """Assembles PREDECLARED_WAVE.json's required_future_run_recording_fields
    for one hypothesis family, from already-persisted trial/benchmark
    artifacts only -- see the module-level docstring above this section."""
    seed = load_seed_universe()

    per_direction: dict[str, dict] = {}
    for direction_key, r, benchmark, long_only in (
        ("long_only", r_lo, benchmark_lo, True),
        ("long_short", r_ls, benchmark_ls, False),
    ):
        targets = rankable_cross_section_targets(
            benchmark["rankable_cross_section_size_by_date"], rank_side_count=RANK_SIDE_COUNT, long_only=long_only
        )
        daily_positions = reconstruct_daily_target_qty(r["folds"], Path(r["economic_returns_csv"]))
        executed = executed_symbol_day_stats(daily_positions)
        actual_gross, actual_net = actual_gross_and_net_exposure_from_positions(daily_positions, bars, EQUITY_USD)
        agg = r["aggregate"]
        per_direction[direction_key] = {
            **targets,
            **executed,
            "desired_gross_exposure": MAX_GROSS_EXPOSURE,
            "desired_net_exposure": MAX_GROSS_EXPOSURE if long_only else 0.0,
            "actual_gross_exposure": actual_gross,
            "actual_net_exposure": actual_net,
            "diagnostic_continuous_average_gross_exposure": agg.get("average_gross_exposure"),
            "turnover": agg.get("total_turnover"),
            "cost_drag": agg.get("cost_drag"),
            "net_return": agg.get("net_total_return"),
            "sharpe": agg.get("net_sharpe"),
            "max_drawdown": agg.get("max_drawdown"),
            "fold_concentration": fold_concentration(r["folds"]),
            "per_date_rankable_cross_section_min_median_max": {
                "min": benchmark["rankable_cross_section_min"],
                "median": benchmark["rankable_cross_section_median"],
                "max": benchmark["rankable_cross_section_max"],
            },
        }

    # R4 (WAVE03-RUN-RECORDING-TRUTH-REPAIR-01): DERIVE holdout_status from
    # each trial's own already-verified "holdout" dict (run_one_trial already
    # fails closed unless it equals RESERVED_NOT_EVALUATED_HOLDOUT_STATUS) --
    # never a bare hardcoded literal here.
    holdout_statuses = {r_lo["holdout"]["status"], r_ls["holdout"]["status"], r_pb["holdout"]["status"]}
    if holdout_statuses != {RESERVED_NOT_EVALUATED_HOLDOUT_STATUS}:
        raise RuntimeError(
            f"Fail-closed: family {fam.key!r} long_only/long_short/placebo holdout status(es) are not "
            f"uniformly {RESERVED_NOT_EVALUATED_HOLDOUT_STATUS!r}: {sorted(holdout_statuses)!r}"
        )

    ever_never = symbols_ever_never_rankable(rankable_by_date, seed_symbols())
    return {
        "family": fam.key,
        "feature_column": fam.feature_column,
        "seed_universe_count": seed["symbol_count"],
        "seed_universe_id": seed["universe_id"],
        **ever_never,
        "first_last_rankable_date_per_symbol": first_last_rankable_date_per_symbol(rankable_by_date),
        "long_only": per_direction["long_only"],
        "long_short": per_direction["long_short"],
        "long_short_minus_long_only_paired_deltas": compute_paired_delta(r_lo, r_ls),
        "matched_placebo_comparison": compute_placebo_delta(r_ls, r_pb),
        "holdout_status": next(iter(holdout_statuses)),
    }


def run_family(family_key: str) -> dict:
    """Real-trial execution for one predeclared hypothesis family: 2 real
    trials (long-only, long-short) plus 1 matched diagnostic placebo trial,
    plus the family-specific dynamic rankable benchmark for each real
    direction, plus every required recording field. Only ever reachable via
    main()'s --execute guard. Reuses the wave-shared bars/features/real-
    targets/placebo-targets caches (ensure_bars/ensure_real_targets/
    ensure_placebo_targets/ensure_full_features)."""
    fam = FAMILIES[family_key]
    bars, manifest = ensure_bars()
    real_targets = ensure_real_targets(bars)
    placebo_targets = ensure_placebo_targets(real_targets)
    full_features = ensure_full_features(bars)
    isolated = isolate_feature(full_features, fam.feature_column)

    fam_root = RUN_ROOT / family_key.lower().replace("-", "_")

    run_dir_lo = fam_root / "long_only"
    bars_path_lo = write_run_dir(run_dir_lo, bars, isolated, real_targets, feature_column=fam.feature_column)
    r_lo = run_one_trial(
        run_dir=run_dir_lo, experiment_id=REAL_EXPERIMENT_ID, hypothesis_id=fam.hyp_long_only,
        strategy_id=fam.strategy_id, direction="long_only", bars_path=bars_path_lo, bars_provenance=manifest,
    )

    run_dir_ls = fam_root / "long_short"
    bars_path_ls = write_run_dir(run_dir_ls, bars, isolated, real_targets, feature_column=fam.feature_column)
    r_ls = run_one_trial(
        run_dir=run_dir_ls, experiment_id=REAL_EXPERIMENT_ID, hypothesis_id=fam.hyp_long_short,
        strategy_id=fam.strategy_id, direction="long_short", bars_path=bars_path_ls, bars_provenance=manifest,
    )

    run_dir_pb = fam_root / "placebo"
    bars_path_pb = write_run_dir(run_dir_pb, bars, isolated, placebo_targets, feature_column=fam.feature_column)
    r_pb = run_one_trial(
        run_dir=run_dir_pb, experiment_id=PLACEBO_EXPERIMENT_ID, hypothesis_id=fam.hyp_placebo,
        strategy_id=fam.strategy_id, direction="long_short", bars_path=bars_path_pb, bars_provenance=manifest,
    )

    # R4 (WAVE03-RUN-RECORDING-TRUTH-REPAIR-01) additional cheap regression:
    # long_only and long_short share byte-identical classification inputs
    # (same features.csv/targets.csv) and the same model -- their OOS
    # prediction membership must therefore be identical, differing only in
    # downstream economic policy. Fail closed if they ever unexpectedly
    # diverge rather than silently trusting only r_lo's file.
    rankable_by_date_lo = rankable_set_by_date(Path(r_lo["oos_predictions_csv"]))
    rankable_by_date_ls = rankable_set_by_date(Path(r_ls["oos_predictions_csv"]))
    if rankable_by_date_lo != rankable_by_date_ls:
        raise RuntimeError(
            f"Fail-closed: family {family_key!r} long_only and long_short OOS prediction membership "
            "differs despite identical classification inputs/model -- refusing to trust either as "
            "this family's RANKABLE_SET(T) authority"
        )
    rankable_by_date = rankable_by_date_lo

    reference_dates_lo = sorted(pd.read_csv(r_lo["economic_daily_returns_csv"])["date"].astype(str).unique())
    benchmark_lo = build_dynamic_rankable_benchmark(
        bars, Path(r_lo["oos_predictions_csv"]), Path(r_lo["economic_returns_csv"]),
        reference_dates_lo, r_lo["holdout_start_utc"],
    )
    reference_dates_ls = sorted(pd.read_csv(r_ls["economic_daily_returns_csv"])["date"].astype(str).unique())
    benchmark_ls = build_dynamic_rankable_benchmark(
        bars, Path(r_ls["oos_predictions_csv"]), Path(r_ls["economic_returns_csv"]),
        reference_dates_ls, r_ls["holdout_start_utc"],
    )

    recording_fields = compute_family_recording_fields(
        fam, r_lo, r_ls, r_pb, benchmark_lo, benchmark_ls, rankable_by_date, bars,
    )

    result = {
        "family": family_key, "feature_column": fam.feature_column,
        "long_only": r_lo, "long_short": r_ls, "placebo": r_pb,
        "benchmark_long_only": benchmark_lo, "benchmark_long_short": benchmark_ls,
        "recording_fields": recording_fields,
    }
    fam_root.mkdir(parents=True, exist_ok=True)
    (fam_root / "family_result.json").write_text(
        json.dumps(result, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    return result


def _hypothesis_to_unique_trial_ids(store: ResearchResultStore, experiment_id: str) -> dict[str, set[str]]:
    by_hypothesis: dict[str, set[str]] = {}
    for t in store.list_trials(experiment_id=experiment_id):
        by_hypothesis.setdefault(t["hypothesis_id"], set()).add(t["trial_id"])
    return by_hypothesis


def _require_exact_frozen_population(
    store: ResearchResultStore, *, experiment_id: str, expected_hypothesis_ids: list[str], population_label: str,
) -> None:
    """R3 (WAVE03-FROZEN-JUDGE-POPULATION-REPAIR-01): fail closed BEFORE any
    judge/closeout call unless the durable registry holds EXACTLY the
    frozen hypothesis population for `experiment_id` -- no missing
    candidate, no extra/unexpected hypothesis (including a placebo
    hypothesis leaking into the real experiment, or vice versa), and no
    duplicated semantic trial (more than one distinct trial_id) under a
    single expected hypothesis. Retries/attempts on the SAME trial_id are
    unaffected -- they never create a second entry in this hypothesis's
    trial-id set."""
    by_hypothesis = _hypothesis_to_unique_trial_ids(store, experiment_id)
    expected = set(expected_hypothesis_ids)
    actual = set(by_hypothesis.keys())
    missing = expected - actual
    unexpected = actual - expected
    if missing or unexpected:
        raise RuntimeError(
            f"Fail-closed: {population_label} population for experiment_id={experiment_id!r} does not "
            f"exactly match the frozen hypothesis set -- missing={sorted(missing)!r} "
            f"unexpected={sorted(unexpected)!r}"
        )
    duplicated = {h: sorted(ids) for h, ids in by_hypothesis.items() if len(ids) != 1}
    if duplicated:
        raise RuntimeError(
            f"Fail-closed: {population_label} population for experiment_id={experiment_id!r} has more "
            f"than one distinct trial registered under a single frozen hypothesis id -- {duplicated!r}"
        )


def run_family_judge() -> dict:
    """Adapted from SHORT-WAVE-03's WAVE03-FAMILY-JUDGE-01 (R3-repaired,
    WAVE03-FROZEN-JUDGE-POPULATION-REPAIR-01): run build_multiple_testing_judge
    over the REAL_EXPERIMENT_ID population only (hypothesis_id=None -> full
    experiment population -- exactly the 2 frozen real-candidate hypothesis
    IDs (long_only, long_short) for this single RISK-01 family, since
    run_family only ever registers real trials under REAL_EXPERIMENT_ID and
    the matched placebo under the structurally distinct PLACEBO_EXPERIMENT_ID
    -- see run_family's own experiment_id routing).

    BEFORE calling build_multiple_testing_judge, the durable registry is
    inspected and required to hold EXACTLY the frozen 2-candidate real
    population and EXACTLY the frozen 1-hypothesis diagnostic placebo
    population -- see _require_exact_frozen_population. No placebo trial can
    ALSO enter the judge's population even if this precheck passed:
    build_multiple_testing_judge's own registry query is scoped by
    experiment_id, and a placebo trial is never registered under
    REAL_EXPERIMENT_ID in the first place."""
    store = ResearchResultStore(REGISTRY_DB)
    _require_exact_frozen_population(
        store, experiment_id=REAL_EXPERIMENT_ID, expected_hypothesis_ids=REAL_CANDIDATE_HYPOTHESIS_IDS,
        population_label="real candidate",
    )
    _require_exact_frozen_population(
        store, experiment_id=PLACEBO_EXPERIMENT_ID, expected_hypothesis_ids=DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS,
        population_label="diagnostic placebo",
    )

    judge = build_multiple_testing_judge(experiment_id=REAL_EXPERIMENT_ID, registry_db=REGISTRY_DB)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    (RUN_ROOT / "judge_artifact.json").write_text(
        json.dumps(judge, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    return judge


EXECUTE_REQUIRED_STAGES = frozenset({"risk01", "judge"})


def main(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    if not argv:
        print("usage: run_wave.py {check|risk01|judge} [--execute]", file=sys.stderr)
        raise SystemExit(2)
    stage = argv[0]
    executed_flag_present = "--execute" in argv

    assert_driver_agrees_with_predeclaration()

    if stage == "check":
        print("PREDECLARATION_AGREEMENT=PASS")
        print(f"SEED_UNIVERSE_COUNT={len(seed_symbols())}")
        return

    if stage in EXECUTE_REQUIRED_STAGES:
        if not executed_flag_present:
            print(
                f"REFUSED: stage {stage!r} requires the explicit --execute flag "
                "(hard execution guard). This predeclaration is not self-executing.",
                file=sys.stderr,
            )
            raise SystemExit(3)
        stage_key = {"risk01": "RISK-01"}
        if stage in stage_key:
            run_family(stage_key[stage])
            return
        if stage == "judge":
            run_family_judge()
            return

    print(f"unknown stage: {stage!r}", file=sys.stderr)
    raise SystemExit(2)


if __name__ == "__main__":
    main()
