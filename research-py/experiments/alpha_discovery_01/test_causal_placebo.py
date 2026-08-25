"""Negative-control proofs for ALPHA-DISCOVERY-01-CAUSAL-PLACEBO-01.

Proves that `build_causal_placebo_targets` (run_experiment.py) permutes the
(fwd_ret, target) PAIR only within rows sharing the exact same
(end_ts, label_end_ts), and therefore can never move an outcome across a
label horizon or a reserved holdout boundary -- unlike the prior driver's
global `target`-only permutation, which is reconstructed here (as
`_buggy_global_permute_target_only`) purely to demonstrate, on the same
fixture, that it DOES violate holdout isolation (RED), while the repaired
function does not (GREEN).

Uses only synthetic fixture data -- no network calls, no Alpaca access, no
research-py/src modification.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_experiment import PLACEBO_SEED, build_causal_placebo_targets  # noqa: E402

LABEL_RET_THRESHOLD = 0.0


def _buggy_global_permute_target_only(targets: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    """Reconstruction of the ORIGINAL (defective) run_02 placebo: permutes
    `target` globally across the whole dataframe while leaving `end_ts` /
    `label_end_ts` untouched. Used only as a RED negative-control fixture in
    this test file -- not imported from the driver, not used in production.
    """
    rng = np.random.default_rng(seed)
    out = targets.copy().reset_index(drop=True)
    perm = rng.permutation(len(out))
    out["target"] = out["target"].to_numpy()[perm]
    return out


def _make_fixture(n_groups: int = 12, symbols: tuple[str, ...] = ("A", "B", "C", "D", "E")) -> pd.DataFrame:
    """Deterministic synthetic targets frame: `n_groups` distinct
    (end_ts, label_end_ts) horizons, each shared by every symbol (mimics a
    shared trading calendar), plus one singleton group of size 1. fwd_ret is
    a deterministic per-row function of (symbol, group) so pair identity is
    easy to trace after permutation, and target is always internally
    consistent with fwd_ret vs LABEL_RET_THRESHOLD.
    """
    rows = []
    for g in range(n_groups):
        end_ts = f"2020-01-{g + 1:02d} 00:00:00"
        label_end_ts = pd.Timestamp(f"2020-02-{g + 1:02d}T00:00:00+00:00").isoformat()
        for si, sym in enumerate(symbols):
            # deterministic, sign alternates across (group, symbol) so both
            # target classes are well represented (avoids a false-positive
            # fixture where every pair in a group is identical).
            fwd_ret = ((-1) ** (g + si)) * (0.001 * (si + 1) + 0.0001 * g)
            rows.append(
                {
                    "symbol": sym,
                    "end_ts": end_ts,
                    "fwd_ret": fwd_ret,
                    "target": 1 if fwd_ret > LABEL_RET_THRESHOLD else 0,
                    "label_end_ts": label_end_ts,
                }
            )
    # one size-1 group: no other row shares its (end_ts, label_end_ts).
    rows.append(
        {
            "symbol": "SOLO",
            "end_ts": "2099-01-01 00:00:00",
            "fwd_ret": 0.05,
            "target": 1,
            "label_end_ts": pd.Timestamp("2099-02-01T00:00:00+00:00").isoformat(),
        }
    )
    return pd.DataFrame(rows)


@pytest.fixture
def targets() -> pd.DataFrame:
    return _make_fixture()


def test_symbol_end_ts_keys_unchanged(targets: pd.DataFrame) -> None:
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    pd.testing.assert_series_equal(placebo["symbol"], targets["symbol"])
    pd.testing.assert_series_equal(placebo["end_ts"], targets["end_ts"])


def test_label_end_ts_unchanged_for_every_row(targets: pd.DataFrame) -> None:
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    pd.testing.assert_series_equal(placebo["label_end_ts"], targets["label_end_ts"])


def test_pair_multiset_preserved_within_each_group(targets: pd.DataFrame) -> None:
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    orig_groups = targets.groupby(["end_ts", "label_end_ts"])
    for key, orig_group in orig_groups:
        placebo_group = placebo[(placebo["end_ts"] == key[0]) & (placebo["label_end_ts"] == key[1])]
        orig_pairs = sorted(zip(orig_group["fwd_ret"], orig_group["target"]))
        placebo_pairs = sorted(zip(placebo_group["fwd_ret"], placebo_group["target"]))
        assert orig_pairs == placebo_pairs, f"pair multiset changed within group {key}"


def test_no_cross_holdout_contamination_repaired(targets: pd.DataFrame) -> None:
    """RED/GREEN: pick a holdout boundary strictly between two group
    horizons. Repaired function must show zero rows whose destination
    label_end_ts < holdout_start received a pair whose TRUE source
    label_end_ts >= holdout_start (or vice versa) -- guaranteed by
    same-group-only permutation, verified explicitly here rather than
    assumed."""
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    label_ends = sorted(targets["label_end_ts"].unique())
    holdout_start = label_ends[len(label_ends) // 2]

    # Build a lookup: for each (fwd_ret, target) pair value, which
    # label_end_ts group(s) did it TRULY originate from in `targets`.
    source_label_end_ts_for_pair: dict[tuple[float, int], set[str]] = {}
    for _, row in targets.iterrows():
        key = (row["fwd_ret"], row["target"])
        source_label_end_ts_for_pair.setdefault(key, set()).add(row["label_end_ts"])

    violations = 0
    for _, row in placebo.iterrows():
        dest_label_end_ts = row["label_end_ts"]
        pair_key = (row["fwd_ret"], row["target"])
        true_sources = source_label_end_ts_for_pair[pair_key]
        dest_side = dest_label_end_ts < holdout_start
        for true_source in true_sources:
            source_side = true_source < holdout_start
            if source_side != dest_side and true_source == dest_label_end_ts:
                # same-group guarantee: a pair's true source label_end_ts
                # must equal the destination's label_end_ts exactly.
                violations += 1
        assert dest_label_end_ts in true_sources, (
            f"pair {pair_key} landed at label_end_ts={dest_label_end_ts} but its true "
            f"source label_end_ts was {true_sources} -- cross-horizon leak"
        )
    assert violations == 0


def test_buggy_global_permute_DOES_cross_holdout_boundary_red_control(targets: pd.DataFrame) -> None:
    """RED control: proves the fixture is capable of exposing the original
    defect. If the reconstructed buggy global permutation produced ZERO
    cross-holdout moves on this fixture, this test (and the fixture) would
    be a false-positive negative control per CLAUDE.md #14 -- so we assert
    the OLD method actually violates isolation here, which is exactly the
    defect PATCH A repairs."""
    buggy = _buggy_global_permute_target_only(targets, seed=PLACEBO_SEED)
    label_ends = sorted(targets["label_end_ts"].unique())
    holdout_start = label_ends[len(label_ends) // 2]

    # `_buggy_global_permute_target_only` draws exactly one
    # rng.permutation(len(out)) call from a freshly-seeded generator, so
    # reconstructing it with the same seed reproduces the exact permutation
    # it applied -- letting us identify each destination row's TRUE source
    # row and compare pre/post-holdout side.
    rng = np.random.default_rng(PLACEBO_SEED)
    perm = rng.permutation(len(targets))
    assert (buggy["target"].to_numpy() == targets["target"].to_numpy()[perm]).all()

    label_end_ts_arr = targets["label_end_ts"].to_numpy()
    dest_is_pre = label_end_ts_arr < holdout_start
    src_is_pre = label_end_ts_arr[perm] < holdout_start
    cross_boundary_moves = int(np.sum(dest_is_pre != src_is_pre))

    assert cross_boundary_moves > 0, (
        "fixture failed to expose the original defect (false-positive negative "
        "control) -- widen n_groups/symbols so the global permutation is proven "
        "capable of crossing the holdout boundary"
    )


def test_no_later_to_earlier_horizon_move(targets: pd.DataFrame) -> None:
    """Invariant 5: no outcome moves from a later label horizon to an
    earlier one. Since permutation is same-group-only, source and
    destination label_end_ts are always identical -- verified directly."""
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    for (et, let), orig_group in targets.groupby(["end_ts", "label_end_ts"]):
        dest_group = placebo[(placebo["end_ts"] == et) & (placebo["label_end_ts"] == let)]
        assert set(dest_group.index) == set(orig_group.index)


def test_some_assignments_actually_change(targets: pd.DataFrame) -> None:
    """Invariant 6: fail closed if the placebo is accidentally identical."""
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    changed = (placebo["target"].to_numpy() != targets["target"].to_numpy()).sum()
    assert changed > 0, "placebo produced zero changed target assignments"


def test_singleton_group_unchanged(targets: pd.DataFrame) -> None:
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    solo_orig = targets[targets["symbol"] == "SOLO"].iloc[0]
    solo_placebo = placebo[placebo["symbol"] == "SOLO"].iloc[0]
    assert solo_orig["fwd_ret"] == solo_placebo["fwd_ret"]
    assert solo_orig["target"] == solo_placebo["target"]


def test_global_positive_label_count_preserved(targets: pd.DataFrame) -> None:
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    assert int(placebo["target"].sum()) == int(targets["target"].sum())


def test_fwd_ret_target_internally_consistent(targets: pd.DataFrame) -> None:
    """Invariant 8: target == 1 iff fwd_ret > frozen label threshold, for
    every row post-placebo (holds because pairs are swapped together, never
    fwd_ret or target independently)."""
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    expected = (placebo["fwd_ret"] > LABEL_RET_THRESHOLD).astype(int)
    assert (placebo["target"] == expected).all()


def test_all_identical_targets_in_group_fails_closed() -> None:
    """RED/GREEN for ALPHA-DISCOVERY-01-PLACEBO-EFFECTIVENESS-FAIL-CLOSED-01:
    a valid input can contain a multi-row same-(end_ts, label_end_ts) group
    whose target values are ALL identical. The RNG can then permute
    rows/fwd_ret while every target assignment stays unchanged -- an
    ineffective negative control, since the classifier consumes `target`.
    The driver must raise RuntimeError rather than silently returning it.
    Fails against the pre-repair 8367c8dc implementation (which returns
    normally here) and passes after the repair.
    """
    end_ts = "2020-01-01 00:00:00"
    label_end_ts = pd.Timestamp("2020-02-01T00:00:00+00:00").isoformat()
    all_same_target_group = pd.DataFrame(
        [
            {"symbol": sym, "end_ts": end_ts, "fwd_ret": 0.001 * (i + 1), "target": 1, "label_end_ts": label_end_ts}
            for i, sym in enumerate(("A", "B", "C", "D"))
        ]
    )
    with pytest.raises(RuntimeError, match="zero changed target assignments"):
        build_causal_placebo_targets(all_same_target_group, seed=PLACEBO_SEED)


def test_mixed_label_fixture_still_succeeds_and_changes_at_least_one_target(targets: pd.DataFrame) -> None:
    """Confirms the repair does not over-trigger: the normal mixed-label
    fixture (mixed target values within groups) continues to return
    normally and changes at least one target assignment."""
    placebo = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    changed = int((placebo["target"].to_numpy() != targets["target"].to_numpy()).sum())
    assert changed > 0


def test_deterministic_across_calls(targets: pd.DataFrame) -> None:
    p1 = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    p2 = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    pd.testing.assert_frame_equal(p1, p2)
