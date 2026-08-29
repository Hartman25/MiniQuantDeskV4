"""SHORT-WAVE-03-BROAD-DIRECT-RANK-PREDECLARATION-01 -- development-stage
Research experiment driver for three predeclared, genuinely distinct
single-feature OOS-classifier-score cross-sectional rank hypotheses
(RANK-01 momentum-rank, RANK-02 short-term-reversal, RANK-03 gap-reversal),
each run as a matched LONG-ONLY control / LONG-SHORT candidate pair plus a
causal same-horizon placebo, using the existing accepted Research ->
registered walk-forward -> economic evaluation -> multiple-testing-judge
pipeline, over the 88-symbol broad Research universe seed (see
SEED_UNIVERSE.json) instead of SHORT-WAVE-02's 12-ETF fixed universe.

All experiment identity, seed universe, data window, label definition,
walk-forward spec, model hyperparameters, cost model, execution model,
rank_side_count, and the placebo seed are FROZEN in PREDECLARED_WAVE.json
BEFORE any trial result is observed. This file must not be edited to react
to an observed result.

CLASSIFICATION (mission): this wave is POST_HOC_DIRECT_RANK_AND_BROAD_
UNIVERSE_DEVELOPMENT_STUDY, not a fresh untouched alpha hypothesis --
direct rank was motivated by SHORT-01/SHORT-WAVE-02's observed classifier-
threshold short-book sparsity, and the universe was widened after seeing
those same results. Maximum possible positive verdict:
MECHANISM_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION. Never
PROVEN_ALPHA or PROMOTION_READY.

HARD EXECUTION GUARD: no stage in EXECUTE_REQUIRED_STAGES may run unless
the literal string "--execute" is present in argv (see main()). The `check`
stage never touches the network and never reads Alpaca credentials.

DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01-CONTROLLER predeclares this wave
and explicitly does NOT run it -- see the mission's "NO REAL EXECUTION IN
THIS CONTROLLER" section. This file exists so the harness is complete
enough to run LATER, under a separate, explicitly-authorized RUN mission.

Uses ONLY existing, frozen, already-accepted production entry points:
  - mqk_research.data.alpaca_historical.extract_research_bars_with_provenance
  - mqk_research.features.feature_set_v1.build_feature_set_v1
  - mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
  - mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge
  - mqk_research.universe.snapshot (SEED_UNIVERSE.json provenance, Patch D)

No research-py/src file is modified by this script.

The causal placebo helper (`build_causal_placebo_targets`) is an exact,
self-contained reproduction of the accepted, PUSHED-VERIFIED
research-alpha-gap-discovery-01-clean worktree's implementation, inlined
the same way SHORT-WAVE-02's own run_wave.py inlined it (portability
convention) so this driver is runnable from a bare checkout of this branch
alone. Semantics unchanged.

BENCHMARK: mission requires a family-specific DYNAMIC equal-weight
comparator (see PREDECLARED_WAVE.json "benchmark") rather than
SHORT-WAVE-02's fixed-12-symbol build_fold_reset_benchmark. Its FORMULA is
predeclared in PREDECLARED_WAVE.json; per the mission ("If implementation
of this benchmark requires new behavior later: STOP before interpreting
Wave-03 results. Do not invent a benchmark after seeing returns."),
`build_dynamic_rankable_benchmark` below is intentionally left as a
NotImplementedError stub -- implementing/validating it against real
fold-reset dates is the separately-reviewed Wave-03 RUN mission's job, not
this predeclaration controller's.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any


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
PRIMARY_PAPER_HEAD_EXPECTED = "edcda740b2f05fbe8a2657f2301b8ea373efb4b6"

REAL_EXPERIMENT_ID = "SHORT-WAVE-03-BROAD-DIRECT-RANK-REAL-V1"
PLACEBO_EXPERIMENT_ID = "SHORT-WAVE-03-BROAD-DIRECT-RANK-PLACEBOS-V1"

START_UTC = pd.Timestamp("2016-01-01T00:00:00Z")
END_UTC = pd.Timestamp("2024-01-01T00:00:00Z")
ASOF = "2024-01-01"
TIMEFRAME = "1Day"
FEED = "sip"

LABEL_HORIZON_BARS = 10
LABEL_RET_THRESHOLD = 0.0

RANK_SIDE_COUNT = 5
MAX_GROSS_EXPOSURE = 1.0
PLACEBO_SEED = 1234

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
    "RANK-01": HypothesisFamily(
        key="RANK-01", feature_column="ret_rank_20",
        strategy_id="pooled_single_feature_xs_momentum_rank_direct_rank_v1",
        hyp_long_only="wave03_rank01_momentum_rank_long_only_v1",
        hyp_long_short="wave03_rank01_momentum_rank_long_short_v1",
        hyp_placebo="wave03_rank01_momentum_rank_placebo_v1",
    ),
    "RANK-02": HypothesisFamily(
        key="RANK-02", feature_column="ret_5",
        strategy_id="pooled_single_feature_short_term_reversal_direct_rank_v1",
        hyp_long_only="wave03_rank02_short_term_reversal_long_only_v1",
        hyp_long_short="wave03_rank02_short_term_reversal_long_short_v1",
        hyp_placebo="wave03_rank02_short_term_reversal_placebo_v1",
    ),
    "RANK-03": HypothesisFamily(
        key="RANK-03", feature_column="gap_pct_1",
        strategy_id="pooled_single_feature_gap_reversal_direct_rank_v1",
        hyp_long_only="wave03_rank03_gap_reversal_long_only_v1",
        hyp_long_short="wave03_rank03_gap_reversal_long_short_v1",
        hyp_placebo="wave03_rank03_gap_reversal_placebo_v1",
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
    assert decl["wave_classification"] == "POST_HOC_DIRECT_RANK_AND_BROAD_UNIVERSE_DEVELOPMENT_STUDY"
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
    assert decl["data"]["end_utc"] == "2024-01-01T00:00:00Z"
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
    assert len(REAL_CANDIDATE_HYPOTHESIS_IDS) == 6
    assert decl["diagnostic_placebo_population"] == DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS
    assert len(DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS) == 3

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
    mapped to more than one fold."""
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
    return {"date_set": set(fold_of_date.index.tolist()), "reset_dates": reset_dates}


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

    HOLDOUT SAFETY: every `reference_dates` entry must fall strictly before
    `holdout_start_utc` (fail closed otherwise, mirroring SHORT-WAVE-02); the
    function only ever reads `bars`/`oos_predictions_csv` rows whose date is
    in `reference_dates`, so a holdout-region row is structurally never
    touched even though `bars` and the OOS predictions file may span dates
    beyond it."""
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
    b["daily_ret"] = b.groupby("symbol")["close"].pct_change()

    sorted_dates = sorted(reference_date_set)
    cross_section_size: dict[str, int] = {}
    per_date: dict[str, float] = {}
    for d in sorted_dates:
        syms = rankable.get(d, set())
        cross_section_size[d] = len(syms)
        if not syms:
            per_date[d] = float("nan")
            continue
        day_rets = b.loc[(b["date"] == d) & (b["symbol"].isin(syms)), "daily_ret"].dropna()
        per_date[d] = float(day_rets.mean()) if len(day_rets) else float("nan")

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


def ensure_bars() -> tuple[pd.DataFrame, dict]:
    """Fetch (or reuse this run's own prior fetch of) real Alpaca bars,
    feed=sip, for the frozen seed universe/window/asof. Cache reuse follows
    the CACHE SAFETY policy in PREDECLARED_WAVE.json -- fail closed unless
    exact manifest verification passes; this stub always fetches fresh
    since no verified-cache implementation exists yet in this controller
    (see mission "CACHE SAFETY REPAIR FROM WAVE-02 LESSON" -- no
    existence-only cache path)."""
    from mqk_research.data.alpaca_historical import extract_research_bars_with_provenance

    _load_paper_credentials_into_env()
    result = extract_research_bars_with_provenance(
        symbols=seed_symbols(), start_utc=START_UTC, end_utc=END_UTC,
        timeframe=TIMEFRAME, asof=ASOF, feed=FEED,
    )
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    bars_path = RUN_ROOT / "raw_bars.csv"
    manifest_path = RUN_ROOT / "bars_provenance_manifest.json"
    result["bars"].to_csv(bars_path, index=False)
    manifest_path.write_text(json.dumps(result["manifest"], sort_keys=True, indent=2, default=str), encoding="utf-8")
    return result["bars"], result["manifest"]


def run_family(family_key: str) -> dict:
    """Real-trial execution for one predeclared hypothesis family. Only
    ever reachable via main()'s --execute guard."""
    raise NotImplementedError(
        f"run_family({family_key!r}) intentionally not implemented in this predeclaration "
        "controller -- the full pipeline (feature isolation, per-trial run_registered_economic_"
        "walkforward_eval calls, dynamic benchmark) is the separately-reviewed Wave-03 RUN "
        "mission's job. This stub exists only so the --execute guard below has something real "
        "to gate; it must never be reached from this controller."
    )


def run_family_judge() -> dict:
    raise NotImplementedError(
        "run_family_judge() intentionally not implemented in this predeclaration controller -- "
        "see run_family()."
    )


EXECUTE_REQUIRED_STAGES = frozenset({"rank01", "rank02", "rank03", "judge"})


def main(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    if not argv:
        print("usage: run_wave.py {check|rank01|rank02|rank03|judge} [--execute]", file=sys.stderr)
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
                "(hard execution guard, mission 'NO REAL EXECUTION IN THIS CONTROLLER'). "
                "This predeclaration controller never passes it.",
                file=sys.stderr,
            )
            raise SystemExit(3)
        stage_key = {"rank01": "RANK-01", "rank02": "RANK-02", "rank03": "RANK-03"}
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
