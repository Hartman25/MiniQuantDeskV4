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
"""
from __future__ import annotations

from pathlib import Path

import pandas as pd
import pytest

from mqk_research.ml.economic_walkforward import _resolve_rank_direction_for_frame
from mqk_research.ml.genuine_shuffled_placebo_cli import (
    _SHUFFLE_MODE_CROSS_SECTIONAL_WITHIN_DECISION_TS,
    _SHUFFLE_MODE_WITHIN_FOLD_ROWS,
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
