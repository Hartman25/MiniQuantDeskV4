"""W06-A-P9-GENUINE-SHUFFLED-PLACEBO-CROSS-SECTIONAL-REPAIR-01 -- negative/
mutation proofs that `_shuffle_oos_predictions`'s rank-specific permutation
(scoped to each `(fold, decision_ts)` cross-section) fixes the confirmed
structural incompatibility with `cross_sectional_rank_long_only_v1` /
`cross_sectional_rank_long_short_v1` candidates, without weakening
`_resolve_rank_direction_for_frame`'s boundary-tie safety and without
disturbing the legacy fold-wide shuffle used by non-rank policies.

These tests exercise `_shuffle_oos_predictions` and
`_resolve_rank_direction_for_frame` directly against small, hand-built OOS
frames -- this is the exact load-bearing pair the mission's confirmed defect
lives in, and avoids the cost of a full registered walk-forward run (already
exercised end-to-end, for this same candidate family, by
`research-py/tests/support/build_r3_e2e_fixture.py` via mqk-cli's R3.5 test).

W06-GENUINE-PLACEBO-SCORE-NONTRIVIALITY-REPAIR-01: independent review found
that the identity check above compared PERMUTED ROW INDICES, not SCORE
ASSIGNMENT -- with duplicate score values away from the selection boundary, a
nonidentity row permutation can leave every symbol's score unchanged
(deterministic repro: trial_93, scores=[0.9, 0.7, 0.5, 0.5], row permutation
[0, 1, 3, 2] swaps only the two equal 0.5 rows). The tests below added by
that repair prove the score-level identity check and its no-meaningful-null
fail-closed behavior.
"""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from mqk_research.ml.economic_walkforward import _resolve_rank_direction_for_frame
from mqk_research.ml.genuine_shuffled_placebo_cli import (
    _SHUFFLE_MODE_CROSS_SECTIONAL_WITHIN_DECISION_TS,
    _SHUFFLE_MODE_WITHIN_FOLD_ROWS,
    _placebo_seed,
    _shuffle_oos_predictions,
)

# A percentile-rank-shaped fold: TWO decision timestamps that each score the
# SAME four symbols against the SAME fixed value set {0.9, 0.7, 0.5, 0.1} --
# exactly the "same small, fixed value set across decision dates" geometry a
# cross_sectional_percentile_rank feature produces (see R3.5's own doc
# comment in core-rs/crates/mqk-cli/src/commands/research_replay.rs).
_SYMBOLS = ["A", "B", "C", "D"]
_SCORES_PER_TS = [0.9, 0.7, 0.5, 0.1]
_DECISION_TS = ["2020-01-01T00:00:00+00:00", "2020-01-02T00:00:00+00:00"]

# Deterministically found (see mission A8#8): this trial_id's derived seed
# permutes the fixture below, under the OLD fold-wide algorithm, into a
# decision frame with a manufactured boundary tie -- while the repaired
# cross-sectional algorithm does not. Not cherry-picked for any other
# property; any trial_id could in principle collide, this one is fixed here
# so the test is deterministic rather than a flaky probability argument.
_TIE_REPRO_TRIAL_ID = "trial_1"


def _clean_fold_frame(fold: int = 0) -> pd.DataFrame:
    rows = []
    for ts in _DECISION_TS:
        for sym, score in zip(_SYMBOLS, _SCORES_PER_TS):
            rows.append({"fold": fold, "symbol": sym, "decision_ts": ts, "ml_score": score})
    return pd.DataFrame(rows)


def _tied_fold_frame(fold: int = 0) -> pd.DataFrame:
    """Top boundary genuinely tied (two symbols at 0.9) at EVERY decision
    timestamp in the original, unshuffled frame."""
    rows = []
    for ts in _DECISION_TS:
        for sym, score in zip(_SYMBOLS, [0.9, 0.9, 0.5, 0.1]):
            rows.append({"fold": fold, "symbol": sym, "decision_ts": ts, "ml_score": score})
    return pd.DataFrame(rows)


def _write(df: pd.DataFrame, path: Path) -> Path:
    df.to_csv(path, index=False)
    return path


def _frame_scores_by_ts(csv_path: Path) -> dict:
    df = pd.read_csv(csv_path)
    return {
        ts: {r.symbol: r.ml_score for r in g.itertuples()}
        for ts, g in df.groupby("decision_ts")
    }


# ---------------------------------------------------------------------------
# A8#1 / A8#2 -- determinism and seed sensitivity (rank path)
# ---------------------------------------------------------------------------


def test_rank_shuffle_same_trial_same_input_is_identical(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    info_1 = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_1.csv", is_rank=True)
    info_2 = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_2.csv", is_rank=True)
    assert info_1 == info_2
    pd.testing.assert_frame_equal(
        pd.read_csv(tmp_path / "out_1.csv"), pd.read_csv(tmp_path / "out_2.csv")
    )


def test_rank_shuffle_different_trial_id_yields_different_assignment(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_a.csv", is_rank=True)
    _shuffle_oos_predictions(oos_path, "trial_b", tmp_path / "out_b.csv", is_rank=True)
    out_a = pd.read_csv(tmp_path / "out_a.csv")
    out_b = pd.read_csv(tmp_path / "out_b.csv")
    assert not out_a["ml_score"].equals(out_b["ml_score"])


# ---------------------------------------------------------------------------
# A8#3 / A8#4 / A8#5 -- multiset preservation, no cross-ts/fold leakage
# ---------------------------------------------------------------------------


def test_rank_shuffle_preserves_exact_score_multiset_per_decision_ts(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=True)
    original = pd.read_csv(oos_path)
    shuffled = pd.read_csv(tmp_path / "out.csv")
    for ts in _DECISION_TS:
        orig_multiset = sorted(original.loc[original["decision_ts"] == ts, "ml_score"].tolist())
        shuf_multiset = sorted(shuffled.loc[shuffled["decision_ts"] == ts, "ml_score"].tolist())
        assert orig_multiset == pytest.approx(shuf_multiset)


def test_rank_shuffle_never_moves_a_score_across_decision_ts_or_fold(tmp_path):
    # Two folds, each with its own two decision timestamps, sharing the SAME
    # score value set -- if a score ever crossed a (fold, decision_ts)
    # boundary, some other group's multiset would change.
    df = pd.concat([_clean_fold_frame(fold=0), _clean_fold_frame(fold=1)], ignore_index=True)
    oos_path = _write(df, tmp_path / "oos.csv")
    _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=True)
    shuffled = pd.read_csv(tmp_path / "out.csv")
    for fold in (0, 1):
        for ts in _DECISION_TS:
            group = shuffled[(shuffled["fold"] == fold) & (shuffled["decision_ts"] == ts)]
            assert sorted(group["ml_score"].tolist()) == pytest.approx(sorted(_SCORES_PER_TS))
    # Row identity (fold, symbol, decision_ts) set is completely unchanged --
    # only the ml_score column may differ.
    original = pd.read_csv(oos_path)
    key_cols = ["fold", "symbol", "decision_ts"]
    assert sorted(map(tuple, original[key_cols].to_numpy().tolist())) == sorted(
        map(tuple, shuffled[key_cols].to_numpy().tolist())
    )


# ---------------------------------------------------------------------------
# A8#6 -- nontriviality (never silently the identity permutation)
# ---------------------------------------------------------------------------


def test_rank_shuffle_is_never_identity_for_a_nontrivial_cross_section(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=True)
    original = pd.read_csv(oos_path)
    shuffled = pd.read_csv(tmp_path / "out.csv")
    merged = original.merge(shuffled, on=["fold", "symbol", "decision_ts"], suffixes=("_orig", "_shuf"))
    for ts in _DECISION_TS:
        group = merged[merged["decision_ts"] == ts]
        assert (group["ml_score_orig"] != group["ml_score_shuf"]).any(), (
            f"decision_ts {ts}: permutation was the identity on a group with distinct scores"
        )
    assert info["groups_permuted"] == len(_DECISION_TS)


# ---------------------------------------------------------------------------
# A8#7 / A8#8 -- tie safety: repaired algorithm avoids the manufactured tie
# the old fold-wide algorithm produces for this exact fixture/trial_id.
# ---------------------------------------------------------------------------


def test_old_fold_wide_algorithm_manufactures_a_boundary_tie_repaired_does_not(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")

    _shuffle_oos_predictions(oos_path, _TIE_REPRO_TRIAL_ID, tmp_path / "old.csv", is_rank=False)
    _shuffle_oos_predictions(oos_path, _TIE_REPRO_TRIAL_ID, tmp_path / "new.csv", is_rank=True)

    old_frames = _frame_scores_by_ts(tmp_path / "old.csv")
    new_frames = _frame_scores_by_ts(tmp_path / "new.csv")

    old_tied = False
    for scores in old_frames.values():
        try:
            _resolve_rank_direction_for_frame(scores, rank_side_count=1, long_only=True)
        except RuntimeError as exc:
            assert "tie" in str(exc)
            old_tied = True
    assert old_tied, (
        "fixture/trial_id no longer reproduces the old-algorithm defect -- pick a new "
        "_TIE_REPRO_TRIAL_ID that does (see this file's module docstring)"
    )

    for ts, scores in new_frames.items():
        # Must not raise: the repaired algorithm never manufactures a tie
        # absent from the real, original decision frame at this timestamp.
        _resolve_rank_direction_for_frame(scores, rank_side_count=1, long_only=True)


def test_repaired_rank_frame_traverses_real_boundary_resolution_both_ways(tmp_path):
    """Both the ORIGINAL and the PERMUTED rank frame must pass through the
    real, unmodified `_resolve_rank_direction_for_frame` without a
    newly-manufactured boundary tie."""
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=True)

    for path in (oos_path, tmp_path / "out.csv"):
        for _, scores in _frame_scores_by_ts(path).items():
            direction = _resolve_rank_direction_for_frame(scores, rank_side_count=1, long_only=True)
            assert sum(1 for v in direction.values() if v == 1) == 1


# ---------------------------------------------------------------------------
# A8#9 -- false-positive control: a genuinely tied boundary must still fail
# closed after permutation (the repair must not hide authentic ambiguity).
# ---------------------------------------------------------------------------


def test_genuinely_tied_boundary_still_fails_closed_after_rank_shuffle(tmp_path):
    oos_path = _write(_tied_fold_frame(), tmp_path / "oos_tied.csv")

    for ts, scores in _frame_scores_by_ts(oos_path).items():
        with pytest.raises(RuntimeError, match="tie"):
            _resolve_rank_direction_for_frame(scores, rank_side_count=1, long_only=True)

    _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "shuffled_tied.csv", is_rank=True)
    for ts, scores in _frame_scores_by_ts(tmp_path / "shuffled_tied.csv").items():
        with pytest.raises(RuntimeError, match="tie"):
            _resolve_rank_direction_for_frame(scores, rank_side_count=1, long_only=True)


# ---------------------------------------------------------------------------
# A8#10 -- non-rank policies keep the legacy fold-wide shuffle unchanged.
# ---------------------------------------------------------------------------


def test_non_rank_path_uses_legacy_fold_wide_shuffle_mode(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=False)
    assert info["shuffle_mode"] == _SHUFFLE_MODE_WITHIN_FOLD_ROWS
    assert info["groups_permuted"] == 1  # one fold, not one per decision_ts
    assert info["rows_permuted"] == 8

    rank_info = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_rank.csv", is_rank=True)
    assert rank_info["shuffle_mode"] == _SHUFFLE_MODE_CROSS_SECTIONAL_WITHIN_DECISION_TS
    assert rank_info["groups_permuted"] == len(_DECISION_TS)

    # The non-rank result must not silently have become the cross-sectional
    # algorithm's output.
    non_rank_out = pd.read_csv(tmp_path / "out.csv")["ml_score"].tolist()
    rank_out = pd.read_csv(tmp_path / "out_rank.csv")["ml_score"].tolist()
    assert non_rank_out != rank_out


# ---------------------------------------------------------------------------
# A8#11 -- row-order permutation of the input is not observable in the
# output (canonical ordering makes the result depend only on trial_id and
# each group's own score multiset).
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("is_rank", [True, False])
def test_shuffle_result_is_independent_of_input_row_order(tmp_path, is_rank):
    forward = _clean_fold_frame()
    reversed_df = forward.iloc[::-1].reset_index(drop=True)

    forward_path = _write(forward, tmp_path / "forward.csv")
    reversed_path = _write(reversed_df, tmp_path / "reversed.csv")

    _shuffle_oos_predictions(forward_path, "trial_a", tmp_path / "out_forward.csv", is_rank=is_rank)
    _shuffle_oos_predictions(reversed_path, "trial_a", tmp_path / "out_reversed.csv", is_rank=is_rank)

    canonical_cols = ["fold", "symbol", "decision_ts", "ml_score"]
    out_forward = pd.read_csv(tmp_path / "out_forward.csv").sort_values(
        ["fold", "symbol", "decision_ts"], kind="mergesort"
    ).reset_index(drop=True)[canonical_cols]
    out_reversed = pd.read_csv(tmp_path / "out_reversed.csv").sort_values(
        ["fold", "symbol", "decision_ts"], kind="mergesort"
    ).reset_index(drop=True)[canonical_cols]
    pd.testing.assert_frame_equal(out_forward, out_reversed)


# ---------------------------------------------------------------------------
# W06-GENUINE-PLACEBO-SCORE-NONTRIVIALITY-REPAIR-01 -- score-level identity
# defect regression (mission A5/A6).
# ---------------------------------------------------------------------------

# Deterministically established (see this file's module docstring):
# `_placebo_seed("trial_93")` permutes idx=[0,1,2,3] into [0,1,3,2] --
# swapping only the two rows carrying the tail duplicate value.
_TRIAL_93 = "trial_93"


def _trial_93_frame(fold: int = 0) -> pd.DataFrame:
    ts = "2020-01-01T00:00:00+00:00"
    rows = [
        {"fold": fold, "symbol": sym, "decision_ts": ts, "ml_score": score}
        for sym, score in zip(["A", "B", "C", "D"], [0.9, 0.7, 0.5, 0.5])
    ]
    return pd.DataFrame(rows)


def test_trial_93_row_permutation_preserves_assignment_row_index_check_would_miss_it():
    """Confirms the CONFIRMED DEFECT precondition itself: the deterministic
    row permutation for trial_93 against idx=[0,1,2,3] is [0,1,3,2] -- a
    nonidentity row permutation (fails a `permuted_idx != idx` check) that
    nonetheless swaps only the two equal-valued (0.5) rows, so the SCORE
    ASSIGNMENT is unchanged. This is the exact silent-passthrough the old
    row-index identity check missed."""
    seed = _placebo_seed(_TRIAL_93)
    rng = np.random.default_rng(seed)
    idx = np.array([0, 1, 2, 3])
    permuted_idx = rng.permutation(idx)
    assert not np.array_equal(permuted_idx, idx), "precondition: row permutation is nonidentity"
    scores = np.array([0.9, 0.7, 0.5, 0.5])
    assert np.array_equal(scores[permuted_idx], scores), (
        "precondition: the nonidentity row permutation leaves the score assignment unchanged"
    )


def test_trial_93_repaired_result_changes_a_real_score_assignment(tmp_path):
    oos_path = _write(_trial_93_frame(), tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, _TRIAL_93, tmp_path / "out.csv", is_rank=True)
    original = pd.read_csv(oos_path)
    shuffled = pd.read_csv(tmp_path / "out.csv")

    # At least one symbol must receive a different ml_score.
    merged = original.merge(shuffled, on=["fold", "symbol", "decision_ts"], suffixes=("_orig", "_shuf"))
    assert (merged["ml_score_orig"] != merged["ml_score_shuf"]).any()

    # The exact score multiset for the group is preserved exactly.
    assert sorted(shuffled["ml_score"].tolist()) == pytest.approx(sorted(original["ml_score"].tolist()))

    # The fallback path was exercised, and the group is reported as both
    # meaningfully permutable and successfully changed.
    assert info["identity_groups_corrected"] == 1
    assert info["groups_meaningfully_permutable"] == 1
    assert info["groups_score_changed"] == 1

    # The real rank boundary resolver still accepts the repaired frame (top
    # score 0.9 is unique -- never a manufactured tie at the boundary).
    scores_by_symbol = dict(zip(shuffled["symbol"], shuffled["ml_score"]))
    direction = _resolve_rank_direction_for_frame(scores_by_symbol, rank_side_count=1, long_only=True)
    assert sum(1 for v in direction.values() if v == 1) == 1


def test_all_distinct_frame_score_change_is_deterministic(tmp_path):
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    info_1 = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_1.csv", is_rank=True)
    info_2 = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out_2.csv", is_rank=True)
    assert info_1 == info_2
    assert info_1["groups_meaningfully_permutable"] == len(_DECISION_TS)
    assert info_1["groups_score_changed"] == len(_DECISION_TS)


def test_duplicate_value_away_from_boundary_remains_valid_after_repair(tmp_path):
    """Two symbols tied at rank 2/3 (0.5), well away from the rank-1
    selection boundary (unique 0.9) -- the repair must preserve the
    multiset, still change the assignment, and the real boundary resolver
    must still accept the frame (no tie at the actual selection boundary)."""
    ts = "2020-01-01T00:00:00+00:00"
    df = pd.DataFrame([
        {"fold": 0, "symbol": sym, "decision_ts": ts, "ml_score": score}
        for sym, score in zip(["A", "B", "C", "D"], [0.9, 0.5, 0.5, 0.1])
    ])
    oos_path = _write(df, tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, "trial_dup", tmp_path / "out.csv", is_rank=True)
    shuffled = pd.read_csv(tmp_path / "out.csv")

    assert sorted(shuffled["ml_score"].tolist()) == pytest.approx([0.1, 0.5, 0.5, 0.9])
    assert info["groups_meaningfully_permutable"] == 1
    assert info["groups_score_changed"] == 1

    scores_by_symbol = dict(zip(shuffled["symbol"], shuffled["ml_score"]))
    direction = _resolve_rank_direction_for_frame(scores_by_symbol, rank_side_count=1, long_only=True)
    assert sum(1 for v in direction.values() if v == 1) == 1


def test_every_meaningfully_permutable_group_has_a_changed_score_assignment(tmp_path):
    """Property check across several fixtures/trial_ids: `groups_score_changed`
    must always equal `groups_meaningfully_permutable` -- never less."""
    fixtures = [
        _clean_fold_frame(),
        _trial_93_frame(),
        pd.concat([_clean_fold_frame(fold=0), _clean_fold_frame(fold=1)], ignore_index=True),
    ]
    trial_ids = ["trial_a", "trial_93", "trial_zzz", "trial_1"]
    for i, df in enumerate(fixtures):
        for trial_id in trial_ids:
            oos_path = _write(df, tmp_path / f"oos_{i}_{trial_id}.csv")
            info = _shuffle_oos_predictions(
                oos_path, trial_id, tmp_path / f"out_{i}_{trial_id}.csv", is_rank=True
            )
            assert info["groups_score_changed"] == info["groups_meaningfully_permutable"], (
                f"fixture {i} trial_id {trial_id}: a meaningfully permutable group left its "
                "score assignment unchanged"
            )


def test_all_equal_scores_group_is_not_meaningfully_permutable(tmp_path):
    """A2: a group with >=2 rows but a single distinct ml_score value has no
    assignment that could ever change, so it must not be counted as
    meaningfully permutable, and its scores must pass through unchanged."""
    ts = "2020-01-01T00:00:00+00:00"
    df = pd.DataFrame([
        {"fold": 0, "symbol": sym, "decision_ts": ts, "ml_score": 0.5}
        for sym in ["A", "B", "C", "D"]
    ])
    oos_path = _write(df, tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=True)
    shuffled = pd.read_csv(tmp_path / "out.csv")

    assert info["groups_seen"] == 1
    assert info["groups_meaningfully_permutable"] == 0
    assert info["groups_score_changed"] == 0
    assert info["identity_groups_corrected"] == 0
    assert (shuffled["ml_score"] == 0.5).all()


def test_all_equal_rank_scores_produce_not_evaluable_through_production_wrapper(tmp_path, monkeypatch):
    """Mission A3, exercised through the REAL `_run_shuffled_placebo`
    production wrapper (a real registered rank trial/artifact, real
    registry/hash-binding checks) -- only the shuffle step itself is
    monkeypatched to deterministically simulate the zero-meaningfully-
    permutable-groups case, since forcing a real trained classifier to emit
    exactly-tied floating-point scores would be fragile/flaky."""
    import sys as _sys

    _tests_dir = str(Path(__file__).resolve().parent)
    if _tests_dir not in _sys.path:
        _sys.path.insert(0, _tests_dir)

    import mqk_research.ml.genuine_shuffled_placebo_cli as placebo_cli
    from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
    from mqk_research.ml.economic_walkforward import (
        SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        AnnualizationSpec,
        CostModelSpec,
        EconomicWalkForwardSpec,
        SignalPolicySpec,
    )
    from mqk_research.ml.eval_walkforward import WalkForwardSpec
    from mqk_research.ml.execution_pricing import ExecutionPricingSpec
    from mqk_research.ml.weight_to_share import WeightToShareSpec

    from test_genuine_shuffled_placebo import (
        BASE_SPEC_KW,
        _build_edge_bars,
        _build_full_dataset,
        _synthetic_bars_provenance,
        _write_full_run_dir,
    )

    registry_db = tmp_path / "registry.sqlite3"
    run_dir = tmp_path / "rank_run"
    df = _build_full_dataset(periods_days=560, seed=0)
    _write_full_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_edge_bars(df).to_csv(bars_path, index=False)

    rank_spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True,
            rank_side_count=1,
            max_gross_exposure=1.0,
        ),
        cost_model=CostModelSpec(commission_bps_per_side=1.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
        annualization=AnnualizationSpec(),
    )
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="genuine_placebo_rank.test",
        hypothesis_id="genuine_placebo_rank.hyp",
        strategy_id="research.placebo_rank",
        bars_csv=bars_path,
        economic_spec=rank_spec,
        bars_provenance=_synthetic_bars_provenance(bars_path),
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**BASE_SPEC_KW),
        steps=200,
    )
    out = json.loads(out_path.read_text(encoding="utf-8"))
    trial_id = out["registry"]["trial_id"]
    eval_id = out["ids"]["economic_eval_id"]

    def _fake_shuffle(oos_path, trial_id_, out_path_, *, is_rank):
        assert is_rank is True
        pd.read_csv(oos_path).to_csv(out_path_, index=False)
        return {
            "seed": 0,
            "rows_shuffled": 0,
            "distinct_folds": 0,
            "shuffle_mode": _SHUFFLE_MODE_CROSS_SECTIONAL_WITHIN_DECISION_TS,
            "groups_permuted": 0,
            "rows_permuted": 0,
            "identity_groups_corrected": 0,
            "groups_seen": 5,
            "groups_meaningfully_permutable": 0,
            "groups_score_changed": 0,
        }

    monkeypatch.setattr(placebo_cli, "_shuffle_oos_predictions", _fake_shuffle)

    result = placebo_cli._run_shuffled_placebo(
        registry_db=registry_db, trial_id=trial_id, economic_eval_id=eval_id,
        placebo_out_dir=tmp_path / "placebo",
    )
    assert result["status"] == "not_evaluable"
    assert "meaningfully permutable" in result["reason"]


def test_non_rank_returned_diagnostics_contract_is_unchanged(tmp_path):
    """A4: non-rank output must remain frozen -- the returned info dict must
    carry exactly its original 7 keys, never the new rank-only diagnostics."""
    oos_path = _write(_clean_fold_frame(), tmp_path / "oos.csv")
    info = _shuffle_oos_predictions(oos_path, "trial_a", tmp_path / "out.csv", is_rank=False)
    assert set(info.keys()) == {
        "seed", "rows_shuffled", "distinct_folds", "shuffle_mode",
        "groups_permuted", "rows_permuted", "identity_groups_corrected",
    }
